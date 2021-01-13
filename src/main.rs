mod irc;
use irc::*;

use std::net::{ ToSocketAddrs, };

use tokio::io::{AsyncWriteExt, AsyncReadExt};
use futures_util::future::FutureExt;
use std::collections::{HashMap, HashSet};
use std::time::{Instant, Duration};
use std::cell::RefCell;

use humantime::format_duration;
use std::ops::Add;
use std::io::{BufReader, BufRead};
use rand::prelude::IteratorRandom;
use rand::{Rng, thread_rng};
use std::fmt::Display;
use std::path::Path;
use stopwatch::Stopwatch;
use json::JsonValue;

async fn async_main(args: &clap::ArgMatches<'_>) -> std::io::Result<()> {
    let addr = args.value_of("server")
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

    let current_channel = args.value_of("channel").unwrap().split(|x| x == ',').next().unwrap();

    context.register_handler(CommandCode::PrivMsg, Box::new(Callouthandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(ZeBotAnswerHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(MiscCommandsHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(ErrnoHandler));
    context.register_handler(CommandCode::Unknown, Box::new(UserStatus::new()));

    context.logon();

    while !context.is_shutdown() {
        // Read from server and stdin simultaneously
        let b = async {
            let prompt = format!("{}> ", current_channel);
            stdout.write_all(prompt.as_bytes()).await?;
            stdout.flush().await?;
            let bytes = stdin.read(stdin_buf.as_mut_slice()).await?;

            if bytes == 0 {  // EOF?
                context.quit();
                return Ok::<_, std::io::Error>(());
            }

            let bytes = &stdin_buf[..bytes];

            let x = String::from_utf8_lossy(bytes);
            let x = x.trim_end();

            context.message(current_channel, x);

            Ok(())
        }.fuse();

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
        .arg(clap::Arg::with_name("server")
            .default_value("localhost:6667")
            .short("s")
            .long("server"))
        .arg(clap::Arg::with_name("nick")
            .default_value("ZeBot")
            .short("n")
            .long("nick"))
        .arg(clap::Arg::with_name("user")
            .default_value("The Bot")
            .short("u")
            .long("user"))
        .arg(clap::Arg::with_name("pass")
            .short("p")
            .long("pass"))
        .arg(clap::Arg::with_name("channel")
            .default_value("#zebot-test")
            .short("c")
            .long("channel"))
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
        let nick = nick.replace(|x:char| !x.is_alphanumeric(), "_");
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
) -> impl Iterator {
    let mut state = 0;
    std::iter::from_fn(move || match state {
        0 => {
            state += 1;
            if let Some(ref h) = header {
                Some(format!(",-------[{}]-------", h))
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

        let valid_chars = "_+*#'\"-$&%()[]{}\\;:<>>".bytes().collect::<HashSet<_>>();

        let command = msg.params[1][1..]
            .split_ascii_whitespace()
            .next()
            .unwrap_or_else(|| "");
        if !command
            .bytes()
            .all(|x| x.is_ascii_alphanumeric() || valid_chars.contains(&x))
        {
            eprintln!("Invalid command {}", command);
            return Ok(HandlerResult::Error("Invalid handler".to_string()));
        }

        let path = format!("./handlers/{}", command);
        let path = Path::new(&path);

        if !path.exists() {
            return Ok(HandlerResult::NotInterested);
        }

        let args = msg.params.iter().map(|x| x.to_string()).collect::<Vec<_>>();

        // Simplest json from handler
        // { "lines": [ ... ],
        //   "dst": "nick" | "channel",   # optional
        //   "box": "0"|"1"|true|false,   # optional
        // }

        dbg!(&args);

        let s = Stopwatch::start_new();
        let cmd_output = std::process::Command::new(path).args(&args).output();
        let s = s.elapsed();

        eprintln!("Handler {} completed in {:?}", command, s);

        match cmd_output {
            Ok(p) => {
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
                                    } else {
                                        lines
                                    };

                                    ctx.message(&dst, ",--------");

                                    for l in &lines {
                                        let l = format!("| {}", l.to_string());
                                        ctx.message(&dst, &l);
                                    }
                                    ctx.message(&dst, "`--------");
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
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
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
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        if msg.params.len() < 2 {
            return Ok(HandlerResult::NotInterested);
        }

        match msg.params[1].as_ref().split(" ").next().unwrap_or(msg.params[1].as_ref()) {
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
                ctx.message(msg.get_reponse_destination(&ctx.joined_channels.borrow()).as_str(), &m);
            }
            _ => return Ok(HandlerResult::NotInterested),
        }

        Ok(HandlerResult::Handled)
    }
}

struct ErrnoHandler;

impl MessageHandler for ErrnoHandler {
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        if msg.params.len() < 2 || !msg.params[1].as_ref().starts_with("!errno ") {
            return Ok(HandlerResult::NotInterested);
        }

        if let Some(x) = msg.params[1].as_ref().split(" ").skip(1).next() {
            if let Ok(n) = x.parse::<u32>() {
                let n = n as i32;
                let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());
                let e = std::io::Error::from_raw_os_error(n);
                let e = if e.to_string().starts_with("Unknown error ") {
                    "Unknown error".to_string()
                } else {
                    e.to_string()
                };
                let m = format!("{}: {}", msg.get_nick(), e.to_string());
                ctx.message(&dst, m.as_str());
            }
        }

        Ok(HandlerResult::Handled)
    }
}

#[derive(Debug,Clone, Eq, PartialEq)]
enum UserEvent {
    Joined(Duration),
    Left(Duration),
    NickChangeFrom(String, Duration),
    NickChangeTo(String, Duration),
}

impl UserEvent {
    fn duration(&self) -> Duration {
        match self {
            UserEvent::NickChangeTo(_, d) => *d,
            UserEvent::NickChangeFrom(_, d) => *d,
            _ => Duration::default(),
        }
    }

    fn to_join(&self) -> Self {
        match self {
            UserEvent::Left(d) | UserEvent::NickChangeFrom(_, d) => UserEvent::Joined(*d),
            _ => UserEvent::Joined(Duration::from_secs(0)),
        }
    }

    fn to_left(&self) -> Self {
        match self {
            UserEvent::Joined(d) | UserEvent::NickChangeFrom(_, d) => UserEvent::Left(*d),
            _ => UserEvent::Left(Duration::from_secs(0)),
        }
    }
}

#[derive(Debug)]
struct ChannelUsers {
    users: HashMap<String, (UserEvent, Instant)>,
}

impl ChannelUsers {
    fn join(&mut self, user: &str) {
        let now = Instant::now();
        let e = if let Some(o) = self.users.get(user) {
            (o.0.to_join(), now)
        } else {
            (UserEvent::Joined(Default::default()), now)
        };
        self.users.insert(user.to_string(), e);
    }
    fn leave(&mut self, user: &str) {
        let now = Instant::now();
        let e = if let Some(o) = self.users.get(user) {
            (o.0.to_left(), now)
        } else {
            (UserEvent::Joined(Default::default()), now)
        };
        if let Some(x) = self.users.insert(user.to_string(), e) {
            match x.0 {
                UserEvent::NickChangeFrom(o, _) => { self.leave(&o); }
                UserEvent::NickChangeTo(o, _) => { self.leave(&o); },
                _ => (),
            }
        }
    }
    fn duration(&self, user: &str) -> Duration {
        if let Some(x) = self.users.get(user) {
            let now = Instant::now();
            now.duration_since(x.1)
        } else {
            Default::default()
        }
    }
}

impl Default for ChannelUsers {
    fn default() -> Self {
        ChannelUsers {
            users: HashMap::new()
        }
    }
}

#[derive(Debug)]
struct UserStatus {
    channels: RefCell<HashMap<String, ChannelUsers>>,
}

impl UserStatus {
    fn new() -> Self {
        UserStatus {
            channels: RefCell::new(HashMap::new()),
        }
    }
}

impl MessageHandler for UserStatus {
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        match msg.command {
            CommandCode::Numeric(353) => {
                let mut c = self.channels.borrow_mut();
                // Add all users on join to channel
                let x = c
                    .entry(msg.params[2].to_string())
                    .or_insert(ChannelUsers::default());
                for n in msg.params[3].to_string().split(|x| x == ' ').map(|x| x.trim_start_matches("@")) {
                    x.join(&n.to_string());
                    eprintln!("> User {} joined on ZeBot join!", n);
                }
            }

            CommandCode::Part => {
                let nick = msg.get_nick();
                let channel = msg.params[0].to_string();
                let mut c = self.channels.borrow_mut();
                let x = c
                    .entry(channel)
                    .or_insert(ChannelUsers::default());
                x.leave(&nick);
                eprintln!("> User {} left", &nick);
            },

            CommandCode::Quit => {
                let nick = msg.get_nick();
                for c in self.channels.borrow_mut().iter_mut() {
                    c.1.leave(&nick);
                }
                eprintln!("> User {} quit", &nick);
            },

            CommandCode::Nick => {
                let nick = msg.get_nick();
                let new_nick = msg.params[0].to_string();
                let mut c = self.channels.borrow_mut();
                let now = Instant::now();
                for x in c.values_mut() {
                    if let Some(u) = x.users.get(&nick).cloned() {
                        // Add old duration too ...
                        let since = now.duration_since(u.1).add(u.0.duration());
                        let since = Duration::from_secs(since.as_secs());
                        x.users.insert(new_nick.clone(), (UserEvent::NickChangeFrom(nick.clone(), since), now));
                        x.users.insert(nick.clone(), (UserEvent::NickChangeTo(new_nick.clone(), since), now));
                    };
                }
                eprintln!("> User {} changed nick to {}", &nick, &new_nick);
            },

            CommandCode::Join => {
                let nick = msg.get_nick();
                let channel = msg.params[0].to_string();
                let mut c = self.channels.borrow_mut();
                let x = c
                    .entry(channel)
                    .or_insert(ChannelUsers::default());
                x.join(&nick);
                eprintln!("> User {} joined", &nick);
            },

            CommandCode::PrivMsg => {
                let nick = msg.get_nick();
                if msg.params[1].starts_with("!status-debug") {
                    eprintln!("{:#?}", &self.channels.borrow());
                } else if msg.params[1].starts_with("!status ") {
                    let qnick = msg.params[1][8..].trim();
                    if !qnick.is_empty() {
                        let qnick = String::from(qnick);
                        let channel = msg.params[0].to_string();
                        self.channels.borrow().get(&channel).map(|cu| {
                            if let Some(u) = cu.users.get(&qnick) {
                                let dur = Instant::now().checked_duration_since(u.1).unwrap();
                                let jp = match &u.0 {
                                    UserEvent::Joined(_d) => {
                                        let dur = format_duration(Duration::from_secs(dur.as_secs()));
                                        format!("{}, {} was here for {}", nick, qnick, dur)
                                    },
                                    UserEvent::Left(_d) => {
                                        let dur = format_duration(Duration::from_secs(dur.as_secs()));
                                        format!("{}, {} was last seen {} ago", nick, qnick, dur)
                                    },
                                    UserEvent::NickChangeFrom(o, d) => {
                                        let d = format_duration(Duration::from_secs(d.as_secs() + dur.as_secs()));
                                        let dur = format_duration(Duration::from_secs(dur.as_secs()));
                                        format!("{}, {} last changed their nick from {} about {} ago, they where seen for {}", nick, qnick, o, dur, d)
                                    },
                                    UserEvent::NickChangeTo(o, d) => {
                                        let d = format_duration(Duration::from_secs(d.as_secs() + dur.as_secs()));
                                        let dur = format_duration(Duration::from_secs(dur.as_secs()));
                                        format!("{}, {} last changed their nick to {} about {} ago, they where seen for {}", nick, qnick, o, dur, d)
                                    },
                                };
                                ctx.message(msg.params[0].as_ref(), &jp);
                            } else {
                                let m = format!("{}, I don't know who {} is", nick, qnick);
                                ctx.message(msg.params[0].as_ref(), &m);
                            }
                        });
                    }
                }
            },
            _ => (),
        }
        Ok(HandlerResult::NotInterested)
    }
}
