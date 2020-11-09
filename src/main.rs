use std::{
    borrow::Cow,
    fmt::{Display, Formatter},
    io::Read,
};

use nom::bytes::complete::{is_not, take_while, take_while_m_n};
use nom::character::complete::none_of;
use nom::character::{is_alphabetic, is_digit};
use nom::combinator::{cond, eof, map, peek};
use nom::lib::std::string::ParseError;
use nom::multi::many_till;
use nom::{
    branch::alt,
    bytes::complete::{tag, take, take_until},
    character::complete::{alpha1, digit1},
    combinator::{opt, rest},
    error::Error,
    multi::{count, many1},
    FindToken, IResult, Parser,
};
use std::process::exit;

struct IRCMessage<'a> {
    prefix: Cow<'a, str>,
    data: &'a [u8],
    command: &'a [u8],
    params: Vec<Cow<'a, str>>,
}

impl<'a> Display for IRCMessage<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Message {{ {}, {}, {:?} }}",
            self.prefix,
            String::from_utf8_lossy(self.command),
            self.params,
        )
    }
}

// // :hitchcock.freenode.net NOTICE * :*** Looking up your hostname...
// // :hitchcock.freenode.net NOTICE * :*** Checking Ident
// // :hitchcock.freenode.net NOTICE * :*** Couldn't look up your hostname
// // :hitchcock.freenode.net NOTICE * :*** No Ident response

// fn message(i:&[u8]) -> std::io::Result<()> {
//     let (i, pfx) = if i[0] == b':' {  // got prefix
//         let prefix_len = i[1..].iter().take_while(|&x| *x != b' ').count();
//         (&i[prefix_len+1..], &i[1..prefix_len+1])
//     } else {
//         (i, &i[0..0])
//     };
//     println!("prefix: '{}'", String::from_utf8_lossy(pfx));
//     Ok(())
// }

fn message<'a>(i: &'a [u8]) -> IResult<&'a [u8], IRCMessage> {
    let (r, i) = take_until("\r\n")(i)?;
    let (r, _) = tag("\r\n")(r)?;

    let m = i;

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

    // let ps = tag(":").and(rest);

    fn middle(i: &[u8]) -> IResult<&[u8], &[u8]> {
        let (i, m) = take_while(|x| x != b' ' && x != 0 && x != b'\r' && x != b'\n')(i)?;
        Ok((i, m))
    }

    fn param(i: &[u8]) -> IResult<&[u8], &[u8]> {
        if let Ok((i, _)) = tag::<_, _, Error<_>>(" ")(i) {
            if let Ok((i, _)) = tag::<_, _, Error<_>>(":")(i) {
                let (i, trailing) = take_while(|x| x != 0 && x != b'\r' && x != b'\n')(i)?;
                Ok((i, trailing))
            } else {
                middle(i)
            }
        } else {
            Ok((i, &i[0..0]))
        }
    }

    let (i, params) = map(many_till(param, eof), |x| {
        x.0.into_iter()
            .filter(|x| !x.is_empty())
            .map(String::from_utf8_lossy)
            .collect::<Vec<_>>()
    })(i)?;

    Ok((
        r,
        IRCMessage {
            prefix: pfx.unwrap_or_default(),
            data: m,
            command,
            params,
        },
    ))
}

enum StreamID<T> {
    Stdin(T),
    Server(T),
}

use async_std::prelude::*;

use async_std::{
    self as astd,
    io,
    stream::StreamExt,
    task,
    net::ToSocketAddrs,
};

async fn async_main() -> std::io::Result<()> {
    let mut addr = "irc.freenode.net:6667".to_socket_addrs()
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
        let mut bytes = {
            let a = async {
                let off = if !remaining_buf.is_empty() {
                    &mut serve_buf[..remaining_buf.len()].copy_from_slice(remaining_buf.as_slice());
                    let off = remaining_buf.len();
                    remaining_buf.clear();
                    off
                } else {
                    0
                };
                let bytes = connection.read(&mut serve_buf.as_mut_slice()[off..]).await?;
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

                            println!("msg: {}", msg);
                            println!("remain_buf.len: {}", i.len());

                            if count == 2 {
                                connection
                                    .write_all(b"USER ZeBot none none :ZeBot\r\n")
                                    .await?;
                                connection.write_all(b"NICK ZeBot\r\n").await?;
                            }
                        },

                        Err(e) => {
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
                let x = String::from_utf8_lossy(buf);
                let x = x.trim_end();
                println!("Got from stdin: {}", x);
            }
        }

    }
    Ok(())
}

fn main() -> std::io::Result<()> {
    task::block_on(async {
        async_main().await
    })
}
