use smol::prelude::*;

use smol::net::{SocketAddr, TcpStream};
use smol::future::block_on;
use std::collections::HashMap;
use smol::io::AsyncWriteExt;
use std::io::{Stdout, Write};
use std::cell::RefCell;

mod message;
pub(crate) use message::*;

mod util;
pub(crate) use util::*;

mod command;
pub(crate) use command::*;

mod handler;
pub use handler::*;

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
