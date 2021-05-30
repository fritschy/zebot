use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;
use std::io::{BufRead, BufReader, Write};
use std::net::ToSocketAddrs;
use std::time::Duration;

use futures_util::future::FutureExt;
use json::JsonValue;
use rand::{Rng, thread_rng};
use rand::prelude::IteratorRandom;
use tokio::io::AsyncReadExt;
use url::Url;

use irc::*;

use clap::crate_version;

mod irc;
mod callout;

use crate::callout::Callouthandler;
use tracing_subscriber::FmtSubscriber;
use tracing::{error as log_error, Level};

pub fn zebot_version() -> String {
    return crate_version!().to_string();
}

async fn async_main(args: &clap::ArgMatches<'_>) -> std::io::Result<()> {
    let addr = args
        .value_of("server")
        .unwrap()
        .to_socket_addrs()?
        .next()
        .expect("Could not resolve host address");

    let mut stdin = tokio::io::stdin();
    let mut stdin_buf = vec![0u8; 1024];

    let nick = args.value_of("nick").unwrap();
    let user = args.value_of("user").unwrap();
    let pass = args.value_of("pass-file").map(|x| String::from(x));
    let mut context = Context::connect(addr, User::new(nick, user), pass).await?;

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
    context.register_handler(CommandCode::Join, Box::new(GreetHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(ZeBotAnswerHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(MiscCommandsHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(SubstituteLastHandler::new()));
    context.register_handler(CommandCode::PrivMsg, Box::new(URLCollector::new()));

    context.logon();

    while !context.is_shutdown() {
        // Read from server and stdin simultaneously
        let stdin_read = async {
            let bytes = stdin.read(stdin_buf.as_mut_slice()).await?;

            if bytes == 0 {
                // EOF?
                context.quit();
                return Err::<(), std::io::Error>(std::io::ErrorKind::BrokenPipe.into());
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
                            log_error!("Error: /MSG Destination Message");
                        } else {
                            context.message(args[0], &args[1..].join(" "));
                        }
                    }

                    "join" => {
                        if args.len() != 1 {
                            log_error!("Error: /JOIN CHANNEL");
                        } else {
                            context.join(args[0]);
                        }
                    }

                    "part" => {
                        if args.len() != 1 {
                            log_error!("Error: /PART CHANNEL");
                        } else {
                            context.leave(args[0]);
                        }
                    }

                    x => {
                        log_error!("Unknown command /{}", x);
                    }
                }
            } else {
                context.message(current_channel, x);
            }

            Ok(())
        }
            .fuse();

        let irc_read = context.update().fuse();

        tokio::pin!(irc_read, stdin_read);

        tokio::select! {
            r = irc_read => {
                match r {
                    Err(e) => {
                        return Err(e);
                    }

                    _ => (),
                }
            }

            r = stdin_read => {
                if let Err(e) = r {
                    return Err(e);
                }
            }

            else => {
                log_error!("Error ...");
                return Err(std::io::ErrorKind::Other.into());
            }
        }
        ;

        // context.update().or(b).await?;
    }

    // One last update to send pending messages...
    context.update().await
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // a builder for `FmtSubscriber`.
    let subscriber = FmtSubscriber::builder()
        // all spans/events with a level higher than TRACE (e.g, debug, info, warn, etc.)
        // will be written to stdout.
        .with_max_level(Level::TRACE)
        // completes the builder.
        .with_thread_names(true)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

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
        .arg(clap::Arg::with_name("pass-file")
            .default_value("password.txt")
            .short("p")
            .long("pass"))
        .arg(
            clap::Arg::with_name("channel")
                .default_value("#zebot-test")
                .short("c")
                .long("channel"),
        )
        .get_matches();

    loop {
        if let Err(x) = async_main(&m).await {
            log_error!("Encountered an error, will retry...: {:?}", x);
        } else {
            log_error!("Exiting as requested, cya.");
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
            log_error!("Could not open nag-file '{}'", &nag_file);
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
        log_error!("Could not open/read nag-file for {}: {:?}", nick, x);
        format!("Hey {}", nick)
    })
}

fn text_box<T: Display, S: Display>(
    mut lines: impl Iterator<Item=T>,
    header: Option<S>,
) -> impl Iterator<Item=String> {
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

struct URLCollector {
    filename: String,
}

impl URLCollector {
    fn new() -> Self {
        URLCollector {
            filename: "rw_data/urls.txt".to_string(),
        }
    }

    fn add_url(&self, nick: &str, chan: &str, url: &str) -> tokio::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .truncate(false)
            .create(true)
            .write(true)
            .read(false)
            .append(true)
            .open(&self.filename)?;

        let line = format!("{}\t{}\t{}\t{}\n", chrono::prelude::Local::now().to_rfc3339(), chan, nick, url);

        f.write_all(line.as_bytes())
    }
}

impl MessageHandler for URLCollector {
    fn handle<'a>(
        &self,
        ctx: &Context,
        msg: &Message<'a>,
    ) -> Result<HandlerResult, std::io::Error> {
        let text = &msg.params[1];

        for word in text.split(" ") {
            match Url::parse(word) {
                Ok(url) => {
                    match url.scheme() {
                        "http" | "https" | "ftp" => {
                            let nick = msg.get_nick();
                            let chan = msg.get_reponse_destination(&ctx.joined_channels.borrow());
                            log_error!("Got an url from {} {}: {}", &chan, &nick, url.as_ref());
                            self.add_url(&nick, &chan, url.as_ref())?;
                        }
                        _ => (),
                    }
                }

                Err(_) => (),
            }
        }

        Ok(HandlerResult::NotInterested)
    }
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

fn parse_substitution(re: &str) -> Option<(String, String, String)> {
    let mut s = 0; // state, see below, can only increment
    let mut sep = '\0';
    let mut pat = String::with_capacity(re.len());
    let mut subst = String::with_capacity(re.len());
    let mut flags = String::with_capacity(re.len());
    for c in re.chars() {
        match s {
            0 => {
                if c != 's' && c != 'S' {
                    log_error!("Not a substitution");
                    return None;
                }
                s = 1;
            }

            1 => {
                sep = c;
                s = 2;
            }

            2 => {
                if c == sep {
                    s = 3;
                } else {
                    pat.push(c);
                }
            }

            3 => {
                if c == sep {
                    s = 4;
                } else {
                    subst.push(c);
                }
            }

            4 => match c {
                'g' | '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' | 's' => {
                    flags.push(c);
                }
                _ => {
                    log_error!("Invalid flags");
                    return None;
                }
            },

            _ => {
                log_error!("Invalid state parsing re");
                dbg!(&re, &c, &s);
                return None;
            }
        }
    }

    Some((pat, subst, flags))
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
            if msg.params[1].starts_with("\x01ACTION") {
                log_error!("Ignoring ACTION message");
                return Ok(HandlerResult::NotInterested);
            }
            self.last_msg
                .borrow_mut()
                .insert((dst.clone(), nick.clone()), msg.params[1].to_string());
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

        let (flags, _save_subst) = if let Some(_) = flags.find("s") {
            (flags.replace("s", ""), true)
        } else {
            (flags, false)
        };

        match regex::Regex::new(&pat) {
            Ok(re) => {
                if let Some(last) = self.last_msg.borrow().get(&(dst.clone(), nick.clone())) {
                    let new_msg = if flags.find("g").is_some() {
                        re.replace_all(last, subst.as_str())
                    } else if let Ok(n) = flags.parse::<usize>() {
                        re.replacen(last, n, subst.as_str())
                    } else {
                        re.replace(last, subst.as_str())
                    };

                    if new_msg != last.as_str() {
                        // if save_subst {
                        //     self.last_msg.borrow_mut().insert((dst.clone(), nick.clone()), new_msg.to_string());
                        //     log_error!("{} new last message '{}'", nick, msg.params[1].to_string());
                        // }

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
            "!version" | "!ver" => {
                let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());
                ctx.message(&dst, &format!("I am version {}, let's not talk about it!", crate_version!()));
            }
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

struct GreetHandler;

fn greet(nick: &str) -> String {
    const PATS: &[&str] = &[
        "Hey {}!",
        "Moin {}, o/",
        "Moin {}, \\o",
        "Moin {}, \\o/",
        "Moin {}, _o/",
        "Moin {}, \\o_",
        "Moin {}, o_/",
        "OI, Ein {}!",
        "{}, n'Moin!",
        "{}, grüß Gott, äh - Zeus! Was gibt's denn Neu's?",
    ];

    if let Some(s) = PATS.iter().choose(&mut thread_rng()) {
        return s.to_string().replace("{}", nick);
    }

    String::from("Hey ") + nick
}

impl MessageHandler for GreetHandler {
    fn handle<'a>(
        &self,
        ctx: &Context,
        msg: &Message<'a>,
    ) -> Result<HandlerResult, std::io::Error> {
        if *ctx.nick() != msg.get_nick() {
            if let CommandCode::Join = msg.command {
                ctx.message(&msg.get_reponse_destination(&ctx.joined_channels.borrow()),
                            &greet(&msg.get_nick()),
                );
            }
        }

        Ok(HandlerResult::NotInterested)
    }
}
