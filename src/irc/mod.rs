use smol::prelude::*;
use smol::net::{SocketAddr, TcpStream};
use smol::io::AsyncWriteExt;

use std::collections::HashMap;
use std::io::{Stdout, Write};
use std::cell::{RefCell, Cell};

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
    fn new() -> Self {
        ReaderBuf {
            buf: RefCell::new(vec![0; 4096]),
            last: RefCell::new(Vec::new())
        }
    }

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
    pub user: User,
    pub channels: RefCell<Vec<String>>,
    pub joined_channels: RefCell<Vec<String>>,
    handlers: HashMap<CommandCode, Vec<Box<dyn MessageHandler>>>,
    allmsg_handlers: Vec<Box<dyn MessageHandler>>,
    pub connection: RefCell<TcpStream>,
    bufs: ReaderBuf,
    messages: RefCell<Vec<String>>,
    shutdown: Cell<bool>,
}

impl Context {
    pub async fn connect(addr: SocketAddr, user: User) -> Result<Self, std::io::Error> {
        let c = TcpStream::connect(addr).await?;
        c.set_nodelay(true)?;

        let connection = RefCell::new(c);

        let mut handlers: HashMap<CommandCode, Vec<Box<dyn MessageHandler>>> = HashMap::new();
        handlers.insert(CommandCode::Ping, vec![Box::new(PingHandler)]);

        let mut allmsg_handlers: Vec<Box<dyn MessageHandler>> = Vec::new();
        allmsg_handlers.push(Box::new(PrintMessageHandler::new()));

        Ok(Context {
            bufs: ReaderBuf::new(),
            channels: RefCell::new(Vec::new()),
            joined_channels: RefCell::new(Vec::new()),
            messages: RefCell::new(Vec::new()),
            shutdown: Cell::new(false),
            allmsg_handlers,
            connection,
            handlers,
            user,
        })
    }

    pub fn nick(&self) -> &String {
        &self.user.nick
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

    pub fn logon(&self) {
        let msg = format!(
            "USER {} none none :The Bot\r\nNICK :{}\r\n",
            self.user.nick,
            self.user.nick,
        );

        println!("Logging on with {} as {}", self.user.user, self.user.nick);

        self.send(msg);
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.get()
    }

    pub fn quit(&self) {
        self.shutdown.replace(true);
        self.messages.borrow_mut().clear();
        self.send("QUIT :Need to restart the Kubernetes VM\r\n".to_string());
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
            self.handlers
                .entry(code)
                .or_insert(Vec::with_capacity(1))
                .push(h);
        }
    }

    pub async fn update(&self) -> Result<(), std::io::Error> {
        // Join channels we want to join...
        if !self.channels.borrow().is_empty() {
            let joins = self.channels.borrow().iter().fold(String::new(), |acc, x| {
                format!("{}JOIN :{}\r\n", acc, x)
            });
            self.joined_channels.borrow_mut().append(&mut self.channels.borrow_mut());
            self.send(joins);
        }

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

        let bytes = self.bufs.read_from(&mut self.connection.borrow_mut()).await?;

        let mut i = &self.bufs.buf.borrow()[..bytes];
        loop {
            match message(i) {
                Ok((r, msg)) => {
                    i = r;

                    for h in self.allmsg_handlers.iter() { h.handle(self, &msg)?; }

                    self.handlers.get(&msg.command).map(|x| -> Result<(), std::io::Error> {
                        for h in x.iter() {
                            match h.handle(self, &msg)? {
                                HandlerResult::Error(x) => eprintln!("Message handler errored: {}", x),
                                HandlerResult::Handled => break,  // Really?
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
