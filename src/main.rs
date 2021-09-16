use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;
use std::io::{BufRead, BufReader, Write, Error};
use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};

use chrono::prelude::*;

use futures_util::future::FutureExt;
use json::JsonValue;
use rand::{Rng, thread_rng};
use rand::prelude::IteratorRandom;
use tokio::io::AsyncReadExt;
use url::Url;

use tracing::info;

use irc::*;

use clap::crate_version;

mod irc;
mod callout;

use crate::callout::Callouthandler;
use tracing_subscriber::FmtSubscriber;
use tracing::{error as log_error, Level};
use irc2::{Message, Prefix};
use futures::executor::block_on;
use std::borrow::Borrow;

pub fn zebot_version() -> String {
    // See build.rs
    let rev_info = env!("GIT_REV_INFO");
    if rev_info != "0" {
        format!("{} {}", crate_version!(), rev_info)
    } else {
        crate_version!().to_string()
    }
}

async fn async_main(args: &clap::ArgMatches<'_>) -> std::io::Result<()> {
    info!("This is ZeBot {}", zebot_version());

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
    let pass = args.value_of("pass-file").map(String::from);
    let mut context = Context::connect(addr, User::new(nick, user), pass).await?;

    for i in args.value_of("channel").unwrap().split(|x| x == ',') {
        context.join(i).await;
    }

    let current_channel = args
        .value_of("channel")
        .unwrap()
        .split(|x| x == ',')
        .next()
        .unwrap();

    context.register_handler(CommandCode::PrivMsg, Box::new(YoutubeTitleHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(Callouthandler));
    context.register_handler(CommandCode::Join, Box::new(GreetHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(ZeBotAnswerHandler::new()));
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

            if let Some(x) = x.strip_prefix('/') {
                let mut cmd_and_args = x.split_whitespace();
                let cmd = cmd_and_args.next().unwrap().trim();
                let args = cmd_and_args.collect::<Vec<_>>();

                match cmd.to_lowercase().as_str() {
                    "msg" => {
                        if args.is_empty() {
                            log_error!("Error: /MSG Destination Message");
                        } else {
                            context.message(args[0], &args[1..].join(" "));
                        }
                    }

                    "join" => {
                        if args.len() != 1 {
                            log_error!("Error: /JOIN CHANNEL");
                        } else {
                            context.join(args[0]).await;
                        }
                    }

                    "part" => {
                        if args.len() != 1 {
                            log_error!("Error: /PART CHANNEL");
                        } else {
                            context.leave(args[0]).await;
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
                if let Err(e) = r {
                    return Err(e);
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
    jv.as_bool().unwrap_or(false) || jv.as_number().unwrap_or_else(|| 0.into()) != 0
}

struct YoutubeTitleHandler;

impl MessageHandler for YoutubeTitleHandler {
    fn handle(&self, ctx: &Context, msg: &Message) -> Result<HandlerResult, Error> {
        if msg.params.len() > 1 {
            let yt_re = regex::Regex::new(r"https?://((www.)?youtube\.com/watch|youtu.be/)").unwrap();
            for url in msg.params[1]
                .split_ascii_whitespace()
                .filter(|x| x.starts_with("https://") || x.starts_with("http://")) {
                if yt_re.is_match(url) {
                    if let Ok(output) = std::process::Command::new("python3")
                        .current_dir("youtube-dl")
                        .args(&[
                            "-m", "youtube_dl", "--quiet", "--get-title", "--socket-timeout", "5", url,
                        ])
                        .output() {
                        let err = String::from_utf8_lossy(output.stderr.as_ref());
                        if !err.is_empty() {
                            log_error!("Got error from youtube-dl: {}", err);
                            let dst = msg.get_reponse_destination(&block_on(async { ctx.joined_channels.read().await }));
                            ctx.message(&dst, &format!("Got an error for URL {}, is this a valid video URL?", &url));
                        } else {
                            let title = String::from_utf8_lossy(output.stdout.as_ref());
                            if !title.is_empty() {
                                let dst = msg.get_reponse_destination(&block_on(async { ctx.joined_channels.read().await }));
                                ctx.message(&dst, &format!("{} has title '{}'", &url, title.trim()));
                            }
                        }
                    }
                } else {
                    // I can't figure out how to not make this one crash with tokio...
                    // use select::document::Document;
                    // use select::predicate::{Class, Name};
                    // let r = reqwest::blocking::get(url);
                    // if let Ok(r) = r {
                    //     if let Ok(b) = r.bytes() {
                    //         let s = String::from_utf8_lossy(b.as_ref());
                    //         let d = Document::from(s.as_ref());
                    //         if let Some(h1) = d.find(Name("h1")).next() {
                    //             let title = h1.text();
                    //             let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());
                    //             ctx.message(&dst, &format!("{} has title '{}'", &url, title.trim()));
                    //         }
                    //     }
                    // }
                }
            }
        }

        Ok(HandlerResult::NotInterested)
    }
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

        let line = format!("{}\t{}\t{}\t{}\n", Local::now().to_rfc3339(), chan, nick, url);

        f.write_all(line.as_bytes())
    }
}

impl MessageHandler for URLCollector {
    fn handle(
        &self,
        ctx: &Context,
        msg: &Message,
    ) -> Result<HandlerResult, std::io::Error> {
        let text = &msg.params[1];

        for word in text.split_ascii_whitespace() {
            if let Ok(url) = Url::parse(word) {
                match url.scheme() {
                    "http" | "https" | "ftp" => {
                        let nick = msg.get_nick();
                        let chan = msg.get_reponse_destination(&block_on( async { ctx.joined_channels.read().await }));
                        log_error!("Got an url from {} {}: {}", &chan, &nick, url.as_ref());
                        self.add_url(&nick, &chan, url.as_ref())?;
                    }
                    _ => (),
                }
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
    fn handle(
        &self,
        ctx: &Context,
        msg: &Message,
    ) -> Result<HandlerResult, std::io::Error> {
        let nick = msg.get_nick();
        let dst = msg.get_reponse_destination(&block_on(async { ctx.joined_channels.read().await }));

        if !msg.params[1].starts_with("!s") && !msg.params[1].starts_with("!S") {
            if msg.params[1].starts_with("\x01ACTION") {
                log_error!("Ignoring ACTION message");
                return Ok(HandlerResult::NotInterested);
            }
            self.last_msg
                .borrow_mut()
                .insert((dst, nick), msg.params[1].clone());
            return Ok(HandlerResult::NotInterested);
        }

        let re = &msg.params[1][1..];
        let big_s = msg.params[1].chars().nth(1).unwrap_or('_') == 'S';

        let (pat, subst, flags) = if let Some(x) = parse_substitution(re) {
            x
        } else {
            ctx.message(&dst, "Could not parse substitution");
            return Ok(HandlerResult::Handled);
        };

        let (flags, _save_subst) = if flags.contains('s') {
            (flags.replace("s", ""), true)
        } else {
            (flags, false)
        };

        match regex::Regex::new(&pat) {
            Ok(re) => {
                if let Some(last) = self.last_msg.borrow().get(&(dst.clone(), nick.clone())) {
                    let new_msg = if flags.contains('g') {
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

struct ZeBotAnswerHandler {
    last: RefCell<HashMap<Prefix, Instant>>,
}

impl ZeBotAnswerHandler {
    fn new() -> Self {
        Self {
            last: RefCell::new(HashMap::new()),
        }
    }
}

impl MessageHandler for ZeBotAnswerHandler {
    fn handle(
        &self,
        ctx: &Context,
        msg: &Message,
    ) -> Result<HandlerResult, std::io::Error> {
        if msg.params.len() > 1 && msg.params[1..].iter().any(|x| x.contains(ctx.nick())) {
            let now = Instant::now();
            let mut last = self.last.borrow_mut();
            let pfx = msg.prefix.as_ref().unwrap();
            if last.contains_key(pfx) {
                let last_ts = *last.get(pfx).unwrap();
                last.entry(pfx.clone()).and_modify(|x| *x = now);
                if now.duration_since(last_ts) < Duration::from_secs(2) {
                    return Ok(HandlerResult::NotInterested);
                }
            } else {
                last.entry(pfx.clone()).or_insert_with(|| now);
            }

            // It would seem, I need some utility functions to retrieve message semantics
            let m = if thread_rng().gen_bool(0.93) {
                nag_user(&msg.get_nick())
            } else {
                format!("Hey {}", &msg.get_nick())
            };

            let dst = msg.get_reponse_destination(&block_on(async {ctx.joined_channels.read().await}));
            ctx.message(&dst, &m);
        }

        // Pretend we're not interested
        Ok(HandlerResult::NotInterested)
    }
}

struct MiscCommandsHandler;

impl MessageHandler for MiscCommandsHandler {
fn handle(
    &self,
    ctx: &Context,
    msg: &Message,
) -> Result<HandlerResult, std::io::Error> {
        if msg.params.len() < 2 {
            return Ok(HandlerResult::NotInterested);
        }

        match msg.params[1]
            .split_ascii_whitespace()
            .next()
            .unwrap_or_else(|| msg.params[1].as_ref())
        {
            "!version" | "!ver" => {
                let dst = msg.get_reponse_destination(&block_on(async { ctx.joined_channels.read().await }));
                ctx.message(&dst, &format!("I am version {}, let's not talk about it!", zebot_version()));
            }
            "!help" | "!commands" => {
                let dst = msg.get_reponse_destination(&block_on(async { ctx.joined_channels.read().await }));
                ctx.message(&dst, "I am ZeBot, I can say Hello and answer to !fortune, !bash, !echo and !errno <int>");
            }
            "!echo" => {
                let dst = msg.get_reponse_destination(&block_on(async { ctx.joined_channels.read().await }));
                let m = &msg.params[1];
                if m.len() > 6 {
                    let m = &m[6..];
                    if !m.is_empty() {
                        ctx.message(&dst, m);
                    }
                }
            }
            "!exec" | "!sh" | "!shell" | "!powershell" | "!power-shell" => {
                let m = format!("Na aber wer wird denn gleich, {}", msg.get_nick());
                ctx.message(
                    msg.get_reponse_destination(&block_on(async { ctx.joined_channels.read().await }))
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
    fn handle(
        &self,
        ctx: &Context,
        msg: &Message,
    ) -> Result<HandlerResult, std::io::Error> {
        if *ctx.nick() != msg.get_nick() {
            if let CommandCode::Join = msg.command {
                ctx.message(&msg.get_reponse_destination(&block_on(async { ctx.joined_channels.read().await })),
                            &greet(&msg.get_nick()),
                );
            }
        }

        Ok(HandlerResult::NotInterested)
    }
}
