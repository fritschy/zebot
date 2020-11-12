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
use nom::AsChar;

async fn async_main() -> std::io::Result<()> {
    let addr = std::env::args()
        .skip(1)
        .next()
        .unwrap_or_else(|| "localhost:6667".to_string())
        .to_socket_addrs()?
        .next()
        .expect("Could not resolve host address");
    let stdin = std::io::stdin();
    let mut stdin = Async::<std::io::StdinLock>::new(stdin.lock())?;
    let stdout = std::io::stdout();
    let mut stdout = Async::<std::io::StdoutLock>::new(stdout.lock())?;

    let mut stdin_buf = vec![0u8; 1024];

    let mut context = Context::connect(addr, User::new("ZeBot", "The Bot", None)).await?;

    context.join("#zebot");
    context.register_handler(CommandCode::PrivMsg, Box::new(FortuneHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(QuestionHandler));
    context.register_handler(CommandCode::PrivMsg, Box::new(HelpHandler));

    context.logon().await?;

    while !context.is_shutdown() {
        // Read from server and stdin simultaneously
        let b = async {
            stdout.write_all(b"> ").await?;
            stdout.flush().await?;
            let bytes = stdin.read(stdin_buf.as_mut_slice()).await?;

            if bytes == 0 {
                context.quit();
                return Ok(());
            }

            let bytes = &stdin_buf[..bytes];

            let x = String::from_utf8_lossy(bytes);
            let x = x.trim_end();

            Ok(())
        };

        context.update().race(b).await?;
    }

    // One last update to send pending messages...
    context.update().await
}

fn main() -> std::io::Result<()> {
    block_on(async { async_main().await })
}

struct QuestionHandler;

impl MessageHandler for QuestionHandler {
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        if msg.prefix.is_none() {
            return Ok(HandlerResult::NotInterested);
        }

        if msg.params.len() > 1 && msg.params[1..].iter().any(|x| x.contains("ZeBot")) {
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
        if msg.prefix.is_none() || msg.params.len() < 2 || msg.params[1] != "!fortune" {
            return Ok(HandlerResult::NotInterested);
        }

        let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());

        match std::process::Command::new("fortune").args(&["-n", "500", "-s"]).output() {
            Ok(p) => {
                ctx.message(&dst, " ,--------");
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
                        format!(" | {}", x)
                    }){
                    ctx.message(&dst, line.as_ref());
                }
                ctx.message(&dst, " `--------");
            },
            Err(e) => {
                ctx.message(&dst, e.to_string().as_str());
            },
        }

        Ok(HandlerResult::NotInterested)
    }
}

struct HelpHandler;

impl MessageHandler for HelpHandler {
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        dbg!(&msg);

        if msg.params.len() < 2 {
            return Ok(HandlerResult::NotInterested);
        }

        match msg.params[1].as_ref() {
            "!help" | "!commands" => {
                let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());
                ctx.message(&dst, "I am ZeBot, I can say Hello and answer to !fortune");
            }
            _ => (),
        }

        Ok(HandlerResult::NotInterested)
    }
}
