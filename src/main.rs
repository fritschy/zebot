mod irc;
use irc::*;

use std::net::{ ToSocketAddrs, };

use select::document::Document;
use select::predicate::{Attr, Name, Predicate};
use tokio::io::{AsyncWriteExt, AsyncReadExt};
use futures_util::future::FutureExt;
use std::collections::HashMap;
use std::time::{Instant, Duration};
use std::cell::RefCell;

use humantime::format_duration;
use std::ops::Add;

async fn async_main(args: clap::ArgMatches<'_>) -> std::io::Result<()> {
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

    context.register_handler(CommandCode::PrivMsg, Box::new(FortuneHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(QuestionHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(MiscCommandsHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(ErrnoHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(GermanBashHandler));
    context.register_handler(CommandCode::Unknown, Box::new(UserStatus::new()));

    context.logon();

    while !context.is_shutdown() {
        // Read from server and stdin simultaneously
        let b = async {
            let prompt = format!("{}> ", current_channel);
            stdout.write_all(prompt.as_bytes()).await?;
            stdout.flush().await?;
            let bytes = stdin.read(stdin_buf.as_mut_slice()).await?;

            if bytes == 0 {
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

    async_main(m).await
}

struct QuestionHandler;

impl MessageHandler for QuestionHandler {
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        if msg.params.len() > 1 && msg.params[1..].iter().any(|x| x.contains(ctx.nick())) {
            // It would seem, I need some utility functions to retrieve message semantics
            let m = format!("Hey {}!", msg.get_nick());

            let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());

            ctx.message(&dst, m.as_str());
        }

        // Pretend we're not interested
        Ok(HandlerResult::NotInterested)
    }
}

struct FortuneHandler;

impl MessageHandler for FortuneHandler {
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        if msg.params.len() < 2 || !msg.params[1].starts_with("!fortune") {
            return Ok(HandlerResult::NotInterested);
        }

        let mut args = vec!["-n", "300"];

        args.extend(msg.params[1]
            .split_ascii_whitespace()
            .skip(1)
            .filter_map(|o| {
                if o.starts_with("-") && o.len() > 1 && o[1..].chars().all(|o| o == 's' || o == 'o' || o == 'a') {
                    Some(o)
                } else {
                    None
                }
            }));

        eprintln!("Fortune args: {}", args.iter().fold(String::new(), |acc, &x| format!("{}{},", acc, x)));

        let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());

        match std::process::Command::new("fortune").args(&args).output() {
            Ok(p) => {
                ctx.message(&dst, ",--------");
                for line in p.stdout
                    .as_slice()
                    .split(|x| *x == b'\n')
                    .filter(|&x| x.len() > 0)
                    .map(|x| x.iter().map(|&x| {
                        // FIXME: Yeah this won't end well...
                        if x.is_ascii_control() || x == b'\t' || x == b'\r' {
                            ' '
                        } else {
                            x as char
                        }
                    }).collect::<String>())
                    .map(|x| {
                        format!("| {}", x)
                    }){
                    ctx.message(&dst, line.as_ref());
                }
                ctx.message(&dst, "`--------");
            },
            Err(e) => {
                ctx.message(&dst, e.to_string().as_str());
            },
        }

        Ok(HandlerResult::Handled)
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

struct GermanBashHandler;

impl MessageHandler for GermanBashHandler {
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        if msg.params.len() < 2 || !msg.params[1].as_ref().starts_with("!bash") {
            return Ok(HandlerResult::NotInterested);
        }

        let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());

        for i in 0.. {
            let text: String = match std::process::Command::new("wget")
                    .args(&["-qO-", "-T3", "http://german-bash.org/action/random"])
                    .output() {
                Ok(p) => {
                    String::from_utf8_lossy(p.stdout.as_slice()).into()
                },
                Err(_e) => {
                    return Ok(HandlerResult::Error("Could not fetch bash".to_string()));
                },
            };

            let document = Document::from(text.as_ref());

            // to find the quote ID
            let num = document.find(Attr("class", "quotebox").descendant(Name("a"))).next();
            let qid = num.map(|x| x.attr("name")).flatten().map(|x| x.to_string());

            let qlines = if let Some(first) = document.find(Attr("class", "zitat")).next() {
                first
                    .find(Attr("class", "quote_zeile"))
                    .map(|x| x.text())
                    .filter(|x| !x.trim().is_empty())
            } else {
                eprintln!("Could not parse HTML");
                ctx.message(&dst, "Uhm, did not recognize the HTML ...");
                return Ok(HandlerResult::Handled);
            };

            let lines = qlines.collect::<Vec<_>>();

            if lines.len() < 10 {
                if let Some(qid) = qid {
                    let h = format!(",--------[ {} ]", qid);
                    ctx.message(&dst, &h);
                } else {
                    ctx.message(&dst, ",--------");
                }

                for line in lines.iter() {
                    let line = format!("| {}", line.trim());
                    ctx.message(&dst, &line);
                }

                ctx.message(&dst, "`--------");

                break;
            }

            eprintln!("Need to request another quote, for the {} time", i+1);
        }

        Ok(HandlerResult::NotInterested)
    }
}

#[derive(Debug,Clone, Eq, PartialEq)]
enum UserEvent {
    Joined,
    Left,
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
}

#[derive(Debug)]
struct ChannelUsers {
    users: HashMap<String, (UserEvent, Instant)>,
}

impl ChannelUsers {
    fn join(&mut self, user: &str) {
        let now = Instant::now();
        self.users.insert(user.to_string(), (UserEvent::Joined, now));
    }
    fn leave(&mut self, user: &str) {
        let now = Instant::now();
        if let Some(x) = self.users.insert(user.to_string(), (UserEvent::Left, now)) {
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

            CommandCode::Part | CommandCode::Quit => {
                let nick = msg.get_nick();
                let channel = msg.params[0].to_string();
                let mut c = self.channels.borrow_mut();
                let x = c
                    .entry(channel)
                    .or_insert(ChannelUsers::default());
                x.leave(&nick);
                eprintln!("> User {} left ({})", &nick, msg.command);
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
                    let qnick = &msg.params[1][8..];
                    if !qnick.is_empty() {
                        let qnick = String::from(qnick);
                        let channel = msg.params[0].to_string();
                        self.channels.borrow().get(&channel).map(|cu| {
                            if let Some(u) = cu.users.get(&qnick) {
                                let dur = Instant::now().checked_duration_since(u.1).unwrap();
                                let jp = match &u.0 {
                                    UserEvent::Joined => {
                                        let dur = format_duration(Duration::from_secs(dur.as_secs()));
                                        format!("{}, {} was here for {}", nick, qnick, dur)
                                    },
                                    UserEvent::Left => {
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
