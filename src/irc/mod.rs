use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Stdout, Write};
use std::net::SocketAddr;
use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub(crate) use irc2::command::*;
pub use handler::*;
use tokio::time::{Duration, timeout, sleep};

use tracing::{error as log_error, info, warn};

mod util;

mod handler;

pub struct User {
    pub nick: String,
    pub user: String,
}

impl User {
    pub fn new(nick: &str, user: &str) -> Self {
        User {
            nick: nick.to_string(),
            user: user.to_string(),
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
            self.buf.borrow_mut()[..l.len()].copy_from_slice(l.as_slice());
            let off = l.len();
            l.clear();
            off
        } else {
            0
        }
    }

    fn push_to_last(&self, i: &[u8]) {
        let l = &mut self.last.borrow_mut();
        let len = i.len();
        l.resize(len, 0);
        l[..len].copy_from_slice(i);
    }

    async fn read_from(&self, source: &mut TcpStream) -> Result<usize, std::io::Error> {
        let off = self.fill_from_last();

        let bytes = source
            .read(&mut self.buf.borrow_mut().as_mut_slice()[off..])
            .await?;

        if bytes == 0 {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Read of length 0 from server",
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
    password_file: String,
}

impl Context {
    pub async fn connect(addr: SocketAddr, user: User, password_file: Option<String>) -> Result<Self, std::io::Error> {
        let c = TcpStream::connect(addr).await?;
        c.set_nodelay(true)?;

        let connection = RefCell::new(c);

        let mut handlers: HashMap<CommandCode, Vec<Box<dyn MessageHandler>>> = HashMap::new();
        handlers.insert(CommandCode::Ping, vec![Box::new(PingHandler)]);

        let allmsg_handlers: Vec<Box<dyn MessageHandler>> = Vec::new();
        // XXX: disable print handler, rely on irc2::parse_ng() output.
        // allmsg_handlers.push(Box::new(PrintMessageHandler::new()));

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
            password_file: password_file.unwrap_or_else(|| String::from("password.txt")),
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

        info!("Logging on with {} as {}", self.user.user, self.user.nick);

        self.send(msg);

        if let Err(e) = std::fs::File::open(&self.password_file).and_then(|mut f| {
            let mut pw = String::new();
            f.read_to_string(&mut pw)?;
            self.message("NickServ", &format!("identify {}", pw.trim()));
            Ok(())
        }) {
            warn!("Could not open password file {}: {:?}", &self.password_file, e);
        }
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.get()
    }

    pub fn quit(&self) {
        self.shutdown.replace(true);
        futures::executor::block_on(async {
            loop {
                match self.messages.try_borrow_mut() {
                    Ok(mut msgs) => {
                        msgs.clear();
                        break;
                    }
                    Err(_bme) => {
                        sleep(Duration::from_millis(100)).await
                    }
                }
            }
        });
        self.send("QUIT :Need to restart the Kubernetes VM\r\n".to_string());
    }

    pub fn send(&self, msg: String) {
        futures::executor::block_on(async {
            loop {
                match self.messages.try_borrow_mut() {
                    Ok(mut msgs) => {
                        msgs.push(msg);
                        break;
                    }
                    Err(_) => {
                        sleep(Duration::from_millis(100)).await
                    }
                }
            }
        });
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
                .or_insert_with(|| Vec::with_capacity(1))
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
            sleep(Duration::from_millis(400 + offset + more_time(count))).await;
        }

        self.last_flush.set(Instant::now());

        Ok(())
    }

    pub async fn update(&self) -> Result<(), std::io::Error> {
        if self.shutdown.get() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Connection shutdown requested"
            ));
        }

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

        // try to timeout ...
        let bytes = {
            let conn = &mut self.connection.borrow_mut();
            timeout(Duration::from_secs(5 * 60),
                    self.bufs.read_from(conn)
            ).await??
        };

        let mut i = &self.bufs.buf.borrow()[..bytes];

        loop {
            match irc2::parse(i) {
                Ok((r, msg)) => {
                    i = r;

                    // Take special care for error messages
                    if msg.command == CommandCode::Error {
                        log_error!("Got ERROR message: {}, closing down", msg);
                        self.quit();
                        futures::executor::block_on(async {
                            self.update().await
                        })?;
                        return Err(std::io::ErrorKind::Other.into());
                    }

                    for h in self.allmsg_handlers.iter() {
                        h.handle(self, &msg)?;
                    }

                    self.handlers
                        .get(&msg.command)
                        .map(|x| -> Result<(), std::io::Error> {
                            for h in x.iter() {
                                match h.handle(self, &msg)? {
                                    HandlerResult::Error(x) => {
                                        log_error!("Message handler errored: {}", x)
                                    }
                                    HandlerResult::Handled => break, // Really?
                                    _ => (),
                                }
                            }
                            Ok(())
                        });
                }

                err => {
                    log_error!("Error from irc2::parse: {:?}", err.unwrap_err());
                    self.bufs.push_to_last(i);
                    break;
                }
            }
        }

        Ok(())
    }
}
