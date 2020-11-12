use smol::{
    self,
    Async,
    future::{ block_on, FutureExt, },
    io::{
        AsyncReadExt,
        AsyncWriteExt,
    },
};

mod irc;
use irc::*;

use std::net::{ ToSocketAddrs, };

use reqwest;

use select::document::Document;
use select::predicate::{Attr, Name, Predicate};

async fn async_main(args: clap::ArgMatches<'_>) -> std::io::Result<()> {
    let addr = args.value_of("server")
        .unwrap()
        .to_socket_addrs()?
        .next()
        .expect("Could not resolve host address");

    let stdin = std::io::stdin();
    let mut stdin = Async::<std::io::StdinLock>::new(stdin.lock())?;
    let stdout = std::io::stdout();
    let mut stdout = Async::<std::io::StdoutLock>::new(stdout.lock())?;

    let mut stdin_buf = vec![0u8; 1024];

    let nick = args.value_of("nick").unwrap();
    let user = args.value_of("user").unwrap();
    let pass = args.value_of("pass");
    let mut context = Context::connect(addr, User::new(nick, user, pass)).await?;

    for i in args.value_of("channel").unwrap().split(|x| x == ',') {
        context.join(i);
    }

    let current_channel = args.value_of("channel").unwrap().split(|x| x == ',').next().unwrap();

    context.logon();

    context.register_handler(CommandCode::PrivMsg, Box::new(FortuneHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(QuestionHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(MiscCommandsHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(ErrnoHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(GermanBashHandler));

    while !context.is_shutdown() {
        // Read from server and stdin simultaneously
        let b = async {
            let prompt = format!("{}> ", current_channel);
            stdout.write_all(prompt.as_bytes()).await?;
            stdout.flush().await?;
            let bytes = stdin.read(stdin_buf.as_mut_slice()).await?;

            if bytes == 0 {
                context.quit();
                return Ok(());
            }

            let bytes = &stdin_buf[..bytes];

            let x = String::from_utf8_lossy(bytes);
            let x = x.trim_end();

            context.message(current_channel, x);

            Ok(())
        };

        context.update().race(b).await?;
    }

    // One last update to send pending messages...
    context.update().await
}

fn main() -> std::io::Result<()> {
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
    block_on(async move { async_main(m).await })
}

struct QuestionHandler;

impl MessageHandler for QuestionHandler {
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        if msg.prefix.is_none() {
            return Ok(HandlerResult::NotInterested);
        }

        if msg.params.len() > 1 && msg.params[1..].iter().any(|x| x.contains(ctx.nick())) {
            // It would seem, I need some utility functions to retrieve message semantics
            let m = format!("Hey {}!",
                            msg.prefix
                                .as_ref()
                                .unwrap()
                                .split("!")
                                .next()
                                .unwrap_or(msg.params[0].as_ref()));

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
        if msg.prefix.is_none() || msg.params.len() < 2 || !msg.params[1].starts_with("!fortune") {
            return Ok(HandlerResult::NotInterested);
        }

        let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());

        match std::process::Command::new("fortune").args(&["-asn", "500"]).output() {
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
            _ => (),
        }

        Ok(HandlerResult::NotInterested)
    }
}

struct ErrnoHandler;

impl MessageHandler for ErrnoHandler {
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        if msg.params.len() < 2 {
            return Ok(HandlerResult::NotInterested);
        }

        if msg.params[1].as_ref().starts_with("!errno ") {
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
        }

        Ok(HandlerResult::NotInterested)
    }
}

struct GermanBashHandler;

impl MessageHandler for GermanBashHandler {
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        if msg.params.len() < 2 {
            return Ok(HandlerResult::NotInterested);
        }

        if msg.params[1].as_ref().starts_with("!bash") {
            let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());
            let client = reqwest::blocking::ClientBuilder::new()
                .timeout(std::time::Duration::from_secs(5))
                .build();
            if let Ok(client) = client {
                match client.get("http://german-bash.org/action/random").send() {
                    Ok(r) => {
                        match r.text() {
                            Ok(html) => {
                                let document = Document::from(html.as_str());

                                // to find the quote ID
                                let num = document.find(Attr("class", "quotebox").descendant(Name("a"))).next();
                                let qid = num.map(|x| x.attr("name")).flatten();

                                if let Some(first) = document.find(Attr("class", "zitat")).next() {
                                    if let Some(qid) = qid {
                                        let h = format!(",--------[ {} ]", qid);
                                        ctx.message(&dst, &h);
                                    } else {
                                        ctx.message(&dst, ",--------");
                                    }

                                    for line in first.find(Attr("class", "quote_zeile"))
                                        .map(|x| x.text())
                                        .filter(|x| !x.trim().is_empty()) {
                                        let line = format!("| {}", line.trim());
                                        ctx.message(&dst, &line);
                                    }
                                    ctx.message(&dst, "`--------");
                                } else {
                                    eprintln!("Could not parse HTML");
                                    ctx.message(&dst, "Uhm, did not recognize the HTML ...");
                                }
                            },
                            Err(e) => {
                                ctx.message(&dst, "That did not work as expected...");
                                eprintln!("{:?}", e)
                            },
                        };
                    },
                    Err(e) => {
                        ctx.message(&dst, "That did not work as expected...");
                        eprintln!("{:?}", e)
                    },
                }
            } else {
                ctx.message(&dst, "That did not work as expected...");
                eprintln!("Could not create client")
            }
        }

        Ok(HandlerResult::NotInterested)
    }
}
