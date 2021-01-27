mod irc;
use irc::*;

use std::net::ToSocketAddrs;

use futures_util::future::FutureExt;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use json::JsonValue;
use rand::prelude::IteratorRandom;
use rand::{thread_rng, Rng};
use std::fmt::Display;
use std::io::{BufRead, BufReader};
use std::path::Path;
use stopwatch::Stopwatch;
use std::collections::HashMap;
use std::error::Error;
use std::cell::RefCell;

async fn async_main(args: &clap::ArgMatches<'_>) -> std::io::Result<()> {
    let addr = args
        .value_of("server")
        .unwrap()
        .to_socket_addrs()?
        .next()
        .expect("Could not resolve host address");

    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    let mut stdin_buf = vec![0u8; 1024];

    let nick = args.value_of("nick").unwrap();
    let user = args.value_of("user").unwrap();
    let pass = args.value_of("pass");
    let mut context = Context::connect(addr, User::new(nick, user, pass)).await?;

    for i in args.value_of("channel").unwrap().split(|x| x == ',') {
        context.join(i);
    }

    let current_channel = args
        .value_of("channel")
        .unwrap()
        .split(|x| x == ',')
        .next()
        .unwrap();

    context.register_handler(CommandCode::PrivMsg, Box::new(Callouthandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(ZeBotAnswerHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(MiscCommandsHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(SubstituteLastHandler::new()));

    context.logon();

    while !context.is_shutdown() {
        // Read from server and stdin simultaneously
        let b = async {
            let prompt = format!("{}> ", current_channel);
            stdout.write_all(prompt.as_bytes()).await?;
            stdout.flush().await?;
            let bytes = stdin.read(stdin_buf.as_mut_slice()).await?;

            if bytes == 0 {
                // EOF?
                context.quit();
                return Ok::<_, std::io::Error>(());
            }

            let bytes = &stdin_buf[..bytes];

            let x = String::from_utf8_lossy(bytes);
            let x = x.trim_end();

            if x.starts_with("/") {
                let mut cmd_and_args = x[1..].split_whitespace();
                let cmd = cmd_and_args.next().unwrap();
                let args = cmd_and_args.collect::<Vec<_>>();

                match cmd.to_lowercase().as_str() {
                    "msg" => {
                        if args.len() < 1 {
                            eprintln!("Error: /MSG Destination Message");
                        } else {
                            context.message(args[0], &args[1..].join(" "));
                        }
                    }

                    "join" => {
                        if args.len() != 1 {
                            eprintln!("Error: /JOIN CHANNEL");
                        } else {
                            context.join(args[0]);
                        }
                    }

                    "part" => {
                        if args.len() != 1 {
                            eprintln!("Error: /PART CHANNEL");
                        } else {
                            context.leave(args[0]);
                        }
                    }

                    x => {
                        eprintln!("Unknown command /{}", x);
                    }
                }
            } else {
                context.message(current_channel, x);
            }

            Ok(())
        }
        .fuse();

        let a = context.update().fuse();

        tokio::pin!(a, b);

        tokio::select! {
            _ = a => (),
            _ = b => (),
        };

        // context.update().or(b).await?;
    }

    // One last update to send pending messages...
    context.update().await
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let m = clap::App::new("zebot")
        .about("An IRC Bot")
        .arg(
            clap::Arg::with_name("server")
                .default_value("localhost:6667")
                .short("s")
                .long("server"),
        )
        .arg(
            clap::Arg::with_name("nick")
                .default_value("ZeBot")
                .short("n")
                .long("nick"),
        )
        .arg(
            clap::Arg::with_name("user")
                .default_value("The Bot")
                .short("u")
                .long("user"),
        )
        .arg(clap::Arg::with_name("pass").short("p").long("pass"))
        .arg(
            clap::Arg::with_name("channel")
                .default_value("#zebot-test")
                .short("c")
                .long("channel"),
        )
        .get_matches();

    loop {
        if let Err(x) = async_main(&m).await {
            eprintln!("Encountered an error, will retry...: {:?}", x);
        } else {
            eprintln!("Exiting as requested, cya.");
            break;
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    Ok(())
}

fn nag_user(nick: &str) -> String {
    fn doit(nick: &str) -> Result<String, std::io::Error> {
        let nick = nick.replace(|x: char| !x.is_alphanumeric(), "_");
        let nag_file = format!("nag-{}.txt", nick);
        let f = std::fs::File::open(&nag_file).map_err(|e| {
            eprintln!("Could not open nag-file '{}'", &nag_file);
            e
        })?;
        let br = BufReader::new(f);
        let l = br.lines();
        let m = l
            .choose(&mut thread_rng())
            .unwrap_or_else(|| Ok("...".to_string()))?;
        Ok(format!("Hey {}, {}", nick, m))
    }

    doit(nick).unwrap_or_else(|x| {
        eprintln!("Could not open/read nag-file for {}: {:?}", nick, x);
        format!("Hey {}", nick)
    })
}

fn text_box<T: Display, S: Display>(
    mut lines: impl Iterator<Item = T>,
    header: Option<S>,
) -> impl Iterator<Item = String> {
    let mut state = 0;
    std::iter::from_fn(move || match state {
        0 => {
            state += 1;
            if let Some(ref h) = header {
                Some(format!(",-------[ {} ]", h))
            } else {
                Some(",-------".to_string())
            }
        }

        1 => match lines.next() {
            None => {
                state += 1;
                Some("`-------".to_string())
            }
            Some(ref next) => Some(format!("| {}", next)),
        },

        _ => None,
    })
}

fn is_json_flag_set(jv: &JsonValue) -> bool {
    jv.as_bool().unwrap_or_else(|| false.into()) || jv.as_number().unwrap_or_else(|| 0.into()) != 0
}

struct SubstituteLastHandler {
    last_msg: RefCell<HashMap<(String, String), String>>,
}

impl SubstituteLastHandler {
    fn new() -> Self {
        SubstituteLastHandler {
            last_msg: RefCell::new(HashMap::new()),
        }
    }
}

fn parse_substitution<'a>(re: &'a str) -> Option<(&'a str, &'a str, &'a str)> {
    let mut s = 0;
    let mut sep = '\0';
    let mut pat = 0..0;
    let mut subst = 0..0;
    let mut flags = 0..0;
    for (i, c) in re.chars().enumerate() {
        match s {
            0 => {
                if c != 's' {
                    eprintln!("Not a substitution");
                    return None;
                }
                s = 1;
            }

            1 => {
                if c != '/' && c != '#' && c != ',' && c != ':' {
                    eprintln!("Invalid separator");
                    return None;
                }
                sep = c;
                s = 2;
            }

            2 => {
                if c == sep {
                    s = 3;
                } else {
                    if pat.start == 0 {
                        pat.start = i;
                        pat.end = i;
                    }
                    pat.end += 1;
                }
            }

            3 => {
                if c == sep {
                    s = 4;
                } else {
                    if subst.start == 0 {
                        subst.start = i;
                        subst.end = i;
                    }
                    subst.end += 1;
                }
            }

            4 => {
                match c {
                    'g' | '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' => {
                        if flags.start == 0 {
                            flags.start = i;
                            flags.end = i;
                        }
                        flags.end += 1;
                    }
                    _ => {
                        eprintln!("Invalid flags");
                        return None;
                    }
                }
            }

            _ => {
                eprintln!("Invalid state parsing re");
                return None;
            }
        }
    }

    return Some((&re[pat], &re[subst], &re[flags]))
}

impl MessageHandler for SubstituteLastHandler {
    fn handle<'a>(
        &self,
        ctx: &Context,
        msg: &Message<'a>,
    ) -> Result<HandlerResult, std::io::Error> {
        let nick = msg.get_nick();
        let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());

        if !msg.params[1].starts_with("!s") && !msg.params[1].starts_with("!S") {
            self.last_msg.borrow_mut().insert((dst.clone(), nick.clone()), msg.params[1].to_string());
            dbg!(msg.params[1].to_string());
            return Ok(HandlerResult::NotInterested);
        }

        let re = &msg.params[1][1..];
        let big_s = msg.params[1].chars().skip(1).next().unwrap() == 'S';

        let (pat, subst, flags) = if let Some(x) = parse_substitution(re) {
            x
        } else {
            ctx.message(&dst, "Could not parse substitution");
            return Ok(HandlerResult::Handled);
        };

        match regex::Regex::new(pat) {
            Ok(re) => {
                if let Some(last) = self.last_msg.borrow().get(&(dst.clone(), nick.clone())) {
                    let new_msg = if flags.find("g").is_some() {
                        re.replace_all(last, subst)
                    } else if let Ok(n) = flags.parse::<usize>() {
                        re.replacen(last, n, subst)
                    } else {
                        re.replace(last, subst)
                    };
                    if new_msg != last.as_str() {
                        let new_msg = if big_s {
                            format!("{} meinte: {}", nick, new_msg)
                        } else {
                            new_msg.to_string()
                        };
                        ctx.message(&dst, &new_msg);
                    }
                }
            }

            Err(_) => {
                ctx.message(&dst, "Could not parse regex");
                return Ok(HandlerResult::Handled);
            }
        }

        Ok(HandlerResult::Handled)
    }
}

struct Callouthandler;

impl MessageHandler for Callouthandler {
    fn handle<'a>(
        &self,
        ctx: &Context,
        msg: &Message<'a>,
    ) -> Result<HandlerResult, std::io::Error> {
        if msg.params.len() < 2 || !msg.params[1].starts_with("!") {
            return Ok(HandlerResult::NotInterested);
        }

        let command = msg.params[1][1..]
            .split_ascii_whitespace()
            .next()
            .unwrap_or_else(|| "");
        if !command
            .chars()
            .all(|x| x.is_ascii_alphanumeric() || x == '-' || x == '_')
        {
            eprintln!("CalloutHandler: Invalid command {}", command);
            return Ok(HandlerResult::NotInterested);
        }

        let command = command.to_lowercase();

        let path = format!("./handlers/{}", command);
        let path = Path::new(&path);

        if !path.exists() {
            return Ok(HandlerResult::NotInterested);
        }

        let nick = msg.get_nick();
        let mut args = msg.params.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        args.insert(0, nick); // this sucks

        // Handler args look like this:
        // $srcnick $src(chan,query) "!command[ ...args]"

        // json from handler
        // { "lines": [ ... ],
        //   "dst": "nick" | "channel",   # optional
        //   "box": "0"|"1"|true|false,   # optional
        //   "wrap": "0"|"1"              # optional
        //   "wrap_single_lines": "0"|"1" # optional
        //   "title": "string"            # optional
        //   "link": "string"             # optional
        // }

        dbg!(&args);

        let s = Stopwatch::start_new();
        let cmd = std::process::Command::new(path).args(&args).output();
        let s = s.elapsed();

        eprintln!("Handler {} completed in {:?}", command, s);

        match cmd {
            Ok(p) => {
                if !p.status.success() {
                    let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());
                    eprintln!("Handler failed with code {}", p.status.code().unwrap());
                    dbg!(&p);
                    ctx.message(&dst, "Somehow, that did not work...");
                    return Ok(HandlerResult::Handled);
                }

                if let Ok(response) = String::from_utf8(p.stdout) {
                    dbg!(&response);
                    match json::parse(&response) {
                        Ok(response) => {
                            let dst = if response.contains("dst") {
                                response["dst"].to_string()
                            } else {
                                msg.get_reponse_destination(&ctx.joined_channels.borrow())
                            };

                            if response.contains("error") {
                                dbg!(&response);
                                ctx.message(&dst, "Somehow, that did not work...");
                                return Ok(HandlerResult::Handled);
                            } else {
                                if !is_json_flag_set(&response["box"]) {
                                    for l in response["lines"].members() {
                                        ctx.message(&dst, &l.to_string());
                                    }
                                } else {
                                    let lines = response["lines"]
                                        .members()
                                        .map(|x| x.to_string())
                                        .collect::<Vec<_>>();
                                    let lines = if is_json_flag_set(&response["wrap"])
                                        && lines.iter().map(|x| x.len()).any(|l| l > 80)
                                    {
                                        let nlines = lines.len();

                                        let s = if lines[nlines - 1].starts_with("    ") {
                                            let (lines, last) = lines.split_at(nlines - 1);

                                            let s = lines.concat();
                                            let s = textwrap::fill(&s, 80);

                                            let s = s + "\n";
                                            s + last[0].as_str()
                                        } else {
                                            let s = lines.concat();
                                            textwrap::fill(&s, 80)
                                        };

                                        s.split(|f| f == '\n')
                                            .map(|x| x.to_string())
                                            .collect::<Vec<_>>()
                                    } else if is_json_flag_set(&response["wrap_single_lines"]) {
                                        let mut new_lines = Vec::with_capacity(lines.len());
                                        let opt = textwrap::Options::new(80)
                                            .splitter(textwrap::NoHyphenation)
                                            .subsequent_indent("  ");
                                        for l in lines {
                                            new_lines.extend(
                                                textwrap::wrap(&l, &opt)
                                                    .iter()
                                                    .map(|x| x.to_string()),
                                            );
                                        }
                                        new_lines
                                    } else {
                                        lines
                                    };

                                    // append link if provided
                                    let lines = if let Some(s) = response["link"].as_str() {
                                        let mut lines = lines;
                                        lines.push(format!("    -- {}", s));
                                        lines
                                    } else {
                                        lines
                                    };

                                    for i in text_box(lines.iter(), response["title"].as_str()) {
                                        ctx.message(&dst, &i);
                                    }
                                }
                            }
                        }

                        Err(e) => {
                            // Perhaps have this as a fallback for non-json handlers? What could possibly go wrong!
                            eprintln!(
                                "Could not parse json from handler {}: {}",
                                command, response
                            );
                            eprintln!("Error: {:?}", e);
                        }
                    }
                } else {
                    eprintln!("Could not from_utf8 for handler {}", command);
                }
            }

            Err(e) => {
                eprintln!("Could not execute handler: {:?}", e);
                return Ok(HandlerResult::NotInterested);
            }
        }

        Ok(HandlerResult::Handled)
    }
}

struct ZeBotAnswerHandler;

impl MessageHandler for ZeBotAnswerHandler {
    fn handle<'a>(
        &self,
        ctx: &Context,
        msg: &Message<'a>,
    ) -> Result<HandlerResult, std::io::Error> {
        if msg.params.len() > 1 && msg.params[1..].iter().any(|x| x.contains(ctx.nick())) {
            // It would seem, I need some utility functions to retrieve message semantics
            let m = if thread_rng().gen_bool(0.93) {
                nag_user(&msg.get_nick())
            } else {
                format!("Hey {}", &msg.get_nick())
            };
            let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());
            ctx.message(&dst, &m);
        }

        // Pretend we're not interested
        Ok(HandlerResult::NotInterested)
    }
}

struct MiscCommandsHandler;

impl MessageHandler for MiscCommandsHandler {
    fn handle<'a>(
        &self,
        ctx: &Context,
        msg: &Message<'a>,
    ) -> Result<HandlerResult, std::io::Error> {
        if msg.params.len() < 2 {
            return Ok(HandlerResult::NotInterested);
        }

        match msg.params[1]
            .as_ref()
            .split(" ")
            .next()
            .unwrap_or(msg.params[1].as_ref())
        {
            "!help" | "!commands" => {
                let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());
                ctx.message(&dst, "I am ZeBot, I can say Hello and answer to !fortune, !bash, !echo and !errno <int>");
            }
            "!echo" => {
                let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());
                let m = &msg.params[1].as_ref();
                if m.len() > 6 {
                    let m = m[6..].trim();
                    if !m.is_empty() {
                        ctx.message(&dst, &m);
                    }
                }
            }
            "!exec" | "!sh" | "!shell" | "!powershell" | "!power-shell" => {
                let m = format!("Na aber wer wird denn gleich, {}", msg.get_nick());
                ctx.message(
                    msg.get_reponse_destination(&ctx.joined_channels.borrow())
                        .as_str(),
                    &m,
                );
            }
            _ => return Ok(HandlerResult::NotInterested),
        }

        Ok(HandlerResult::Handled)
    }
}
