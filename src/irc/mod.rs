use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Stdout, Write};
use std::time::{Duration, Instant};

mod message;
pub(crate) use message::*;

mod util;
pub(crate) use util::*;

mod command;
pub(crate) use command::*;

mod handler;
pub use handler::*;
use irc2;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

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
            pass: pass.map(|x| x.to_string()),
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
            last: RefCell::new(Vec::new()),
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
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Read of length 0 fro server",
            ))
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
    last_flush: Cell<Instant>,
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
            last_flush: Cell::new(Instant::now()),
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
        } else {
            let p = self.joined_channels.borrow().iter().position(|x| x == chan);
            if let Some(c) = p {
                self.joined_channels.borrow_mut().remove(c);
                let cmd = format!("PART {}\r\n", chan);
                self.send(cmd);
            }
        }
    }

    pub fn logon(&self) {
        let msg = format!(
            "USER {} none none :The Bot\r\nNICK :{}\r\n",
            self.user.nick, self.user.nick,
        );

        println!("Logging on with {} as {}", self.user.user, self.user.nick);

        self.send(msg);

        if let Err(e) = std::fs::File::open("password.txt").and_then(|mut f| {
            let mut pw = String::new();
            f.read_to_string(&mut pw)?;
            self.message("NickServ", &format!("identify {}", pw.trim()));
            Ok(())
        }) {
            eprintln!("Could not open password.txt: {:?}", e);
        }
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

    async fn send_pending_messages(&self) -> Result<(), std::io::Error> {
        if self.messages.borrow().is_empty() {
            return Ok(());
        }

        let mut connection = self.connection.borrow_mut();

        fn more_time(count: usize) -> u64 {
            if count > 8 {
                (count as u64 - 9) * 100
            } else {
                0
            }
        }

        let offset = if (Instant::now() - self.last_flush.get()).as_millis() < 2000 {
            400
        } else {
            0
        };

        for (count, m) in self.messages.borrow_mut().drain(..).enumerate() {
            connection.write_all(m.as_bytes()).await?;
            // This does not take into account messages sent with the previous commits...
            tokio::time::sleep(Duration::from_millis(400 + offset + more_time(count))).await;
        }

        self.last_flush.set(Instant::now());

        Ok(())
    }

    pub async fn update(&self) -> Result<(), std::io::Error> {
        // Join channels we want to join...
        if !self.channels.borrow().is_empty() {
            let joins = self
                .channels
                .borrow()
                .iter()
                .fold(String::new(), |acc, x| format!("{}JOIN :{}\r\n", acc, x));
            self.joined_channels
                .borrow_mut()
                .append(&mut self.channels.borrow_mut());
            self.send(joins);
        }

        self.send_pending_messages().await?;

        let bytes = self
            .bufs
            .read_from(&mut self.connection.borrow_mut())
            .await?;

        if bytes == 0 {
            self.shutdown.set(true);
            return Ok(());
        }

        let mut i = &self.bufs.buf.borrow()[..bytes];

        // feed the received message to the experimental parser ...
        irc2::parse_ng(i);

        loop {
            match message(i) {
                Ok((r, msg)) => {
                    i = r;

                    for h in self.allmsg_handlers.iter() {
                        h.handle(self, &msg)?;
                    }

                    self.handlers
                        .get(&msg.command)
                        .map(|x| -> Result<(), std::io::Error> {
                            for h in x.iter() {
                                match h.handle(self, &msg)? {
                                    HandlerResult::Error(x) => {
                                        eprintln!("Message handler errored: {}", x)
                                    }
                                    HandlerResult::Handled => break, // Really?
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
