use std::{
    borrow::Cow,
    fmt::{Display, Formatter},
    io::Read,
    net::{SocketAddr, TcpStream, ToSocketAddrs},
};

use smol::{
    block_on, future,
    io::{self, AsyncReadExt},
    unblock, Async, Unblock,
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
use smol::io::AsyncWriteExt;
use smol::stream::StreamExt;
use smol::future::FutureExt;
use std::process::exit;

#[macro_use]
extern crate smol;

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
            String::from_utf8_lossy(self.data),
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
    let (i, command) = alt((take_while(is_alphabetic), take_while_m_n(3, 3, is_digit)))(i)?;

    // let ps = tag(":").and(rest);

    fn middle(i: &[u8]) -> IResult<&[u8], &[u8]> {
        if let Ok((i, _)) = none_of::<_, _, Error<_>>(":")(i) {
            let (i, m) = take_while(|x| x != b' ' && x != 0 && x != b'\r' && x != b'\n')(i)?;
            Ok((i, m))
        } else {
            Ok((i, &i[0..0]))
        }
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

    // let (i, params) = map(many_till(param, eof), |x| {
    //     x.0.into_iter()
    //         .filter(|x| !x.is_empty())
    //         .map(String::from_utf8_lossy)
    //         .collect::<Vec<_>>()
    // })(i)?;

    Ok((
        r,
        IRCMessage {
            prefix: pfx.unwrap_or_default(),
            data: m,
            command,
            params: Vec::new(),
        },
    ))
}

enum StreamID<T> {
    Stdin(T),
    Server(T),
}

fn main() -> std::io::Result<()> {
    block_on(async {
        let mut addr = unblock(move || "irc.freenode.net:6667".to_socket_addrs())
            .await?
            .next()
            .expect("Could not resolve host address");
        let mut connection = Async::<TcpStream>::connect(addr).await?;
        let mut stdin = Unblock::new(std::io::stdin());
        let mut stdout = Unblock::new(std::io::stdout());

        let mut stdin_buf = vec![0u8; 1024];
        let mut serve_buf = vec![0u8; 1024];

        let mut count: usize = 0;
        loop {
            // Read from server and stdin simultaneously
            let mut bytes = {
                for i in stdin_buf.iter_mut() {
                    *i = 0;
                }
                future::race(
                    async {
                        let mut off = 0;
                        loop {
                            let bytes = connection.read(&mut serve_buf[off..]).await?;
                            off += bytes;
                            if &serve_buf[off-2..off] == b"\r\n" {
                                break;
                            }
                            dbg!(&serve_buf[..off]);
                        }
                        Ok::<_, std::io::Error>(StreamID::Server(&serve_buf[..off]))
                    },
                    async {
                        stdout.write_all(b"> ").await?;
                        stdout.flush().await?;
                        let bytes = stdin.read(stdin_buf.as_mut_slice()).await?;
                        Ok::<_, std::io::Error>(StreamID::Stdin(&stdin_buf[..bytes]))
                    }
                ).await?
            };

            match bytes {
                StreamID::Server(buf) => {
                    if let Ok((remain_buf, msg)) = message(buf) {
                        count += 1;

                        println!("msg: {}", msg);

                        if count == 2 {
                            connection
                                .write_all(b"USER ZeBot none none :ZeBot\r\n")
                                .await?;
                            connection.write_all(b"NICK ZeBot\r\n").await?;
                        }
                    } else {
                        eprintln!("Got an error...");
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
    })
}
