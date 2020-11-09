use std::{
    borrow::Cow,
    fmt::{Display, Formatter},
};

use nom::bytes::complete::{take_while, take_while_m_n};
use nom::character::{is_alphabetic, is_digit};
use nom::combinator::{eof, map};
use nom::multi::many_till;
use nom::{
    branch::alt,
    bytes::complete::{tag, take_until},
    combinator::{opt},
    error::Error,
    IResult,
};

use async_std::{self as astd, net::ToSocketAddrs, task, prelude::*};

enum StreamID<T> {
    Stdin(T),
    Server(T),
}

struct IRCMessage<'a> {
    prefix: Cow<'a, str>,
    command: Cow<'a, str>,
    params: Vec<Cow<'a, str>>,
}

impl<'a> Display for IRCMessage<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Message {{ {}, {}, {:?} }}",
            self.prefix,
            self.command,
            self.params,
        )
    }
}

// This is rather messy... see rfc1459
fn message<'a>(i: &'a [u8]) -> IResult<&'a [u8], IRCMessage> {
    let (r, i) = take_until("\r\n")(i)?;
    let (r, _) = tag("\r\n")(r)?;

    fn prefix(i: &[u8]) -> IResult<&[u8], Cow<str>> {
        // let (i, _) = tag::<&str, &[u8], nom::error::Error<&[u8]>>(":")(i)?;
        let (i, _) = tag(":")(i)?;
        let (i, server_or_nick) = take_until(" ")(i)?;
        let (r, _) = tag(" ")(i)?;
        let server_or_nick = String::from_utf8_lossy(server_or_nick);
        Ok((r, server_or_nick))
    }

    let (i, pfx) = opt(prefix)(i)?;
    let (i, command) = alt((take_while_m_n(3, 3, is_digit), take_while(is_alphabetic)))(i)?;
    let command = String::from_utf8_lossy(command);

    fn param(i: &[u8]) -> IResult<&[u8], &[u8]> {
        if let Ok((i, _)) = tag::<_, _, Error<_>>(" ")(i) {
            if let Ok((i, _)) = tag::<_, _, Error<_>>(":")(i) {
                let (i, trailing) = take_while(|x| x != 0 && x != b'\r' && x != b'\n')(i)?;
                Ok((i, trailing))
            } else {
                take_while(|x| x != b' ' && x != 0 && x != b'\r' && x != b'\n')(i)  // middle
            }
        } else {
            Ok((i, &[]))
        }
    }

    let (_, params) = map(many_till(param, eof), |x| {
        x.0.into_iter()
            .filter(|x| !x.is_empty())
            .map(String::from_utf8_lossy)
            .collect::<Vec<_>>()
    })(i)?;

    Ok((
        r,
        IRCMessage {
            prefix: pfx.unwrap_or_default(),
            command,
            params,
        },
    ))
}

async fn async_main(handler: &mut MessageHandler) -> std::io::Result<()> {
    let addr = "irc.freenode.net:6667"
        .to_socket_addrs()
        .await?
        .next()
        .expect("Could not resolve host address");
    let mut connection = astd::net::TcpStream::connect(addr).await?;
    let mut stdin = astd::io::stdin();
    let mut stdout = astd::io::stdout();

    let mut stdin_buf = vec![0u8; 1024];
    let mut serve_buf = vec![0u8; 1024];

    // will contain remains of he last read that could not be parsed as a message...
    let mut remaining_buf = Vec::new();

    let mut count: usize = 0;
    loop {
        // Read from server and stdin simultaneously
        let bytes = {
            let a = async {
                // Need to complete a previous message
                let off = if !remaining_buf.is_empty() {
                    &mut serve_buf[..remaining_buf.len()].copy_from_slice(remaining_buf.as_slice());
                    let off = remaining_buf.len();
                    remaining_buf.clear();
                    off
                } else {
                    0
                };

                let bytes = connection
                    .read(&mut serve_buf.as_mut_slice()[off..])
                    .await?;

                Ok::<_, std::io::Error>(StreamID::Server(&serve_buf[..off + bytes]))
            };

            let b = async {
                stdout.write_all(b"> ").await?;
                stdout.flush().await?;
                let bytes = stdin.read(stdin_buf.as_mut_slice()).await?;
                Ok::<_, std::io::Error>(StreamID::Stdin(&stdin_buf[..bytes]))
            };

            a.race(b).await
        }?;

        match bytes {
            StreamID::Server(buf) => {
                let mut i = buf;
                loop {
                    match message(i) {
                        Ok((r, msg)) => {
                            i = r;
                            count += 1;

                            handler.handle(&mut connection, count, &msg)?;
                        }

                        Err(_) => {
                            remaining_buf.reserve(i.len());
                            for x in i {
                                remaining_buf.push(*x);
                            }
                            break;
                        }
                    }
                }
            }
            StreamID::Stdin(buf) => {
                if buf.is_empty() {
                    connection.write_all(b"QUIT\r\n").await?;
                    connection.shutdown(astd::net::Shutdown::Both)?;
                    break;
                }

                let x = String::from_utf8_lossy(buf);
                let x = x.trim_end();
                println!("Got from stdin: {}", x);
            }
        }
    }

    Ok(())
}

#[derive(Clone, PartialEq)]
enum Channel {
    Name(String),
}

struct MessageHandler {
    nick: String,
    channels: Vec<Channel>,
}

impl MessageHandler {
    fn with_nick(n: &str) -> Self {
        MessageHandler {
            nick: n.to_string(),
            channels: Vec::new(),
        }
    }

    fn channel(mut self, c: Channel) -> Self {
        self.channels.push(c);
        self
    }

    fn handle(
        &mut self,
        ret: &mut astd::net::TcpStream,
        id: usize,
        msg: &IRCMessage,
    ) -> std::io::Result<()> {
        println!(
            "{}: {} {:?}",
            id,
            msg.command,
            msg.params
        );
        match msg.command.as_bytes() {
            b"PING" => {
                let response = format!("PONG {}\r\n", msg.params[0]);
                println!("Sending: {}", response);
                astd::task::block_on(async { ret.write_all(response.as_bytes()).await })
            }

            _ => {
                if id == 1 {
                    // First message: "logon"
                    astd::task::block_on(async {
                        let msg = format!(
                            "USER {} none none :The Bot\r\nNICK {}\r\n{}",
                            self.nick,
                            self.nick,
                            self.channels.iter().fold(String::new(), |acc, x| {
                                match x {
                                    Channel::Name(n) => format!("{}JOIN {}\r\n", acc, n),
                                }
                            })
                        );
                        println!("Sending: {}", msg);
                        ret.write_all(msg.as_bytes()).await
                    })
                } else {
                    Ok(())
                }
            }
        }
    }
}

fn main() -> std::io::Result<()> {
    let mut msg_handler =
        MessageHandler::with_nick("ZeBot").channel(Channel::Name("#zebot-test".to_string()));

    task::block_on(async { async_main(&mut msg_handler).await })
}
