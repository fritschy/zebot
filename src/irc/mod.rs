use std::{
    borrow::Cow,
    fmt::{
        Display,
        Formatter,
    },
};

use smol::prelude::*;

use nom::{
    IResult,
    bytes::complete::{
        tag,
        take_until,
        take_while,
        take_while_m_n,
    },
    multi::many_till,
    character::{
        is_alphabetic,
        is_digit,
    },
    error::Error,
    combinator::{
        opt,
        map,
        eof,
    },
    branch::alt
};
use smol::net::{SocketAddr, TcpStream};
use smol::future::block_on;
use std::collections::HashMap;
use smol::io::AsyncWriteExt;
use std::io::{Stdout, Write};
use std::cell::RefCell;

pub trait Join<T, S = T> {
    fn join(&self, sep: S) -> T;
}

impl<'a> Join<String, &str> for Vec<Cow<'a, str>> {
    fn join(&self, sep: &str) -> String {
        self.iter().fold(String::new(), |acc, x| {
            if acc.is_empty() {
                x.to_string()
            } else {
                acc + sep + x
            }
        })
    }
}

#[derive(Eq, PartialEq, Hash, Debug)]
pub enum CommandCode {
    Numeric(u32),
    Generic(String),  // Yeah ...
    PrivMsg,
    Notice,
    Nick,
    Join,
    Part,
    Quit,
    Mode,
    Ping,
    Unknown,
}

impl<'a> From<Cow<'a, str>> for CommandCode {
    fn from(c: Cow<'a, str>) -> Self {
        if c.len() == 3 && c.as_bytes().iter().all(|x| x.is_ascii_digit()) {
            CommandCode::Numeric(c.as_bytes().iter().rev().enumerate().fold(0u32, |acc, x| {
                acc + (*x.1 - b'0') as u32 * 10u32.pow(x.0 as u32)
            }))
        } else {
            match c.as_bytes() {
                b"PRIVMSG" => CommandCode::PrivMsg,
                b"NOTICE" => CommandCode::Notice,
                b"NICK" => CommandCode::Nick,
                b"JOIN" => CommandCode::Join,
                b"PART" => CommandCode::Part,
                b"QUIT" => CommandCode::Quit,
                b"MODE" => CommandCode::Mode,
                b"PING" => CommandCode::Ping,
                b"UNKNOWN" => CommandCode::Unknown,
                _ => {
                    eprintln!("WARNING: Fallback to generic CommandCode for {}", c);
                    CommandCode::Generic(c.to_string())
                },
            }
        }
    }
}

impl Display for CommandCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandCode::PrivMsg => write!(f, "PRIVMSG")?,
            CommandCode::Notice => write!(f, "NOTICE")?,
            CommandCode::Nick => write!(f, "NICK")?,
            CommandCode::Join => write!(f, "JOIN")?,
            CommandCode::Part => write!(f, "PART")?,
            CommandCode::Quit => write!(f, "QUIT")?,
            CommandCode::Mode => write!(f, "MODE")?,
            CommandCode::Ping => write!(f, "PING")?,
            CommandCode::Unknown => write!(f, "UNKNOWN")?,
            CommandCode::Numeric(n) => write!(f, "{:03}", n)?,
            CommandCode::Generic(n) => write!(f, "{}", n)?,
        }
        Ok(())
    }
}

pub struct Message<'a> {
    pub prefix: Cow<'a, str>,
    pub command: CommandCode,
    pub params: Vec<Cow<'a, str>>,
}

impl<'a> Display for Message<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}, {}, {}",
            self.prefix,
            self.command,
            self.params.join(" "),
        )
    }
}

// This is rather messy... see rfc1459
pub(crate) fn message<'a>(i: &'a [u8]) -> IResult<&'a [u8], Message> {
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
        Message {
            prefix: pfx.unwrap_or_default(),
            command: command.into(),
            params,
        },
    ))
}

pub struct User {
    pub nick: String,
    pub user: String,
    pub pass: Option<String>,
}

impl User {
    pub fn new(nick: &str, user: &str, pass: Option<&str>) -> Self {
        User {
            nick: nick.to_string(),
            user: user.to_string(),
            pass: pass.map(|x| x.to_string())
        }
    }
}

struct PingHandler;

impl MessageHandler for PingHandler {
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        let resp = format!("PONG :{}\r\n", msg.prefix);
        block_on(
            async {
                ctx.connection
                    .borrow_mut()
                    .write_all(resp.as_bytes())
                    .await
            }).map(|_| HandlerResult::Handled)
    }
}

struct PrintMessageHandler {
    stdout: Stdout,
    count: RefCell<usize>,
}

impl PrintMessageHandler {
    fn new() -> Self {
        PrintMessageHandler {
            stdout: std::io::stdout(),
            count: RefCell::new(0),
        }
    }
}

impl MessageHandler for PrintMessageHandler {
    fn handle<'a>(&self, _: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        let mut count = self.count.borrow_mut();
        *count += 1;
        let mut out = self.stdout.lock();
        let m = format!("{:-5}: {}\n", count, msg);
        out.write_all(m.as_bytes())?;
        Ok(HandlerResult::NotInterested)   // pretend to not be interested...
    }
}

struct ReaderBuf {
    buf: RefCell<Vec<u8>>,
    last: RefCell<Vec<u8>>,
}

impl ReaderBuf {
    fn fill_from_last(&self) -> usize {
        let len = self.last.borrow().len();
        if len > 0 {
            let mut l = self.last.borrow_mut();
            &mut self.buf.borrow_mut()[..l.len()].copy_from_slice(l.as_slice());
            let off = l.len();
            l.clear();
            off
        } else {
            0
        }
    }

    async fn read_from(&self, source: &mut TcpStream) -> Result<usize, std::io::Error> {
        let off = self.fill_from_last();

        let bytes = source
            .read(&mut self.buf.borrow_mut().as_mut_slice()[off..])
            .await?;

        if bytes == 0 {
            Err(std::io::Error::new(std::io::ErrorKind::Other, "Read of length 0 fro server"))
        } else {
            Ok(off + bytes)
        }
    }
}

pub struct Context {
    pub server: SocketAddr,
    pub user: User,
    pub channels: RefCell<Vec<String>>,
    pub joined_channels: RefCell<Vec<String>>,
    handlers: HashMap<CommandCode, Vec<Box<dyn MessageHandler>>>,
    allmsg_handlers: Vec<Box<dyn MessageHandler>>,
    pub connection: RefCell<TcpStream>,
    bufs: ReaderBuf,
    messages: RefCell<Vec<String>>,
    shutdown: RefCell<bool>,
    count: RefCell<usize>,
}

impl Context {
    pub async fn connect(addr: SocketAddr, user: User) -> Result<Self, std::io::Error> {
        let connection = RefCell::new(TcpStream::connect(addr).await?);
        connection.borrow_mut().set_nodelay(true)?;

        let mut handlers: HashMap<CommandCode, Vec<Box<dyn MessageHandler>>> = HashMap::new();
        handlers.insert(CommandCode::Ping, vec![Box::new(PingHandler)]);

        let mut allmsg_handlers: Vec<Box<dyn MessageHandler>> = Vec::new();
        allmsg_handlers.push(Box::new(PrintMessageHandler::new()));

        let bufs = ReaderBuf{buf: RefCell::new(vec![0; 4096]), last: RefCell::new(Vec::new())};

        Ok(Context {
            server: addr,
            user,
            channels: RefCell::new(Vec::new()),
            joined_channels: RefCell::new(Vec::new()),
            handlers,
            allmsg_handlers,
            connection,
            bufs,
            messages: RefCell::new(Vec::new()),
            shutdown: RefCell::new(false),
            count: RefCell::new(0),
        })
    }

    pub fn join(&self, chan: &str) {
        self.channels.borrow_mut().push(chan.to_string());
    }

    #[allow(unused)]
    pub fn leave(&self, chan: &str) {
        if let Some(c) = self.channels.borrow().iter().position(|x| x == chan) {
            self.channels.borrow_mut().remove(c);
        } else if let Some(c) = self.joined_channels.borrow().iter().position(|x| x == chan) {
            self.joined_channels.borrow_mut().remove(c);
            let cmd = format!("PART {}\r\n", chan);
            self.send(cmd);
        }
    }

    pub async fn logon(&self) -> Result<(), std::io::Error> {
        let msg = format!(
            "USER {} none none :The Bot\r\nNICK :{}\r\n",
            self.user.nick,
            self.user.nick,
        );

        println!("Logging on with {} as {}", self.user.user, self.user.nick);

        self.connection.borrow_mut().write_all(msg.as_bytes()).await
    }

    pub fn is_shutdown(&self) -> bool {
        *self.shutdown.borrow()
    }

    pub fn quit(&self) {
        *self.shutdown.borrow_mut() = true;
        self.messages.borrow_mut().clear();
        self.send("QUIT\r\n".to_string());
    }

    pub fn send(&self, msg: String) {
        self.messages.borrow_mut().push(msg);
    }

    pub fn message(&self, dst: &str, msg: &str) {
        let msg = format!("PRIVMSG {} :{}\r\n", dst, msg);
        self.send(msg);
    }

    #[allow(unused)]
    pub fn register_handler(&mut self, code: CommandCode, h: Box<dyn MessageHandler>) {
        if let CommandCode::Unknown = code {
            self.allmsg_handlers.push(h);
        } else {
            self.handlers.entry(code).or_insert(vec![h]);
        }
    }

    pub async fn update(&self) -> Result<(), std::io::Error> {
        // Send all queued messages
        self.connection
            .borrow_mut()
            .write_all(
                self.messages
                    .borrow_mut()
                    .drain(..)
                    .fold(String::new(), |acc, x| {
                        format!("{}{}", acc, x)
                    })
                    .as_bytes())
            .await?;

        // Join channels we want to join...
        if !self.channels.borrow().is_empty() {
            let joins = self.channels.borrow().iter().fold(String::new(), |acc, x| {
                format!("{}JOIN :{}\r\n", acc, x)
            });
            self.joined_channels.borrow_mut().append(&mut self.channels.borrow_mut());
            self.send(joins);
        }

        let bytes = self.bufs.read_from(&mut self.connection.borrow_mut()).await?;

        let mut i = &self.bufs.buf.borrow()[..bytes];
        loop {
            match message(i) {
                Ok((r, msg)) => {
                    i = r;

                    *self.count.borrow_mut() += 1;

                    for h in self.allmsg_handlers.iter() { h.handle(self, &msg)?; }

                    self.handlers.get(&msg.command).map(|x| -> Result<(), std::io::Error> {
                        for h in x.iter() {
                            match h.handle(self, &msg)? {
                                HandlerResult::Error(x) => eprintln!("Message handler errored: {}", x),
                                HandlerResult::Handled => break,
                                _ => (),
                            }
                        }
                        Ok(())
                    });
                }

                Err(_) => {
                    let l = &mut self.bufs.last.borrow_mut();
                    l.reserve(i.len());
                    for x in i {
                        l.push(*x);
                    }
                    break;
                }
            }
        }

        Ok(())
    }
}

pub enum HandlerResult {
    Handled,
    NotInterested,
    Error(String),
}

pub trait MessageHandler {
    fn handle(&self, ctx: &Context, msg: &Message) -> Result<HandlerResult, std::io::Error>;
}
