use crate::irc::*;

pub enum HandlerResult {
    Handled,
    NotInterested,
    Error(String),
}

pub trait MessageHandler {
    fn handle(&self, ctx: &Context, msg: &Message) -> Result<HandlerResult, std::io::Error>;
}

pub(crate) struct PingHandler;

impl MessageHandler for PingHandler {
    fn handle<'a>(&self, ctx: &Context, msg: &Message<'a>) -> Result<HandlerResult, std::io::Error> {
        // Freenode sends a prefix, ircdng does not, but has a param we can use.
        let dst = if let Some(prefix) = &msg.prefix {
            prefix
        } else if !msg.params.is_empty() {
            &msg.params[0]
        } else {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "Don't know how to respond to PING w/o params or prefix!"));
        };

        let resp = format!("PONG {} :{}\r\n", dst, dst);

        block_on(
            async {
                ctx.connection
                    .borrow_mut()
                    .write_all(resp.as_bytes())
                    .await
            }).map(|_| HandlerResult::Handled)
    }
}

pub(crate) struct PrintMessageHandler {
    stdout: Stdout,
    count: RefCell<usize>,
}

impl PrintMessageHandler {
    pub(crate) fn new() -> Self {
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
        let m = format!("\t{}: {}\n", count, msg);
        out.write_all(m.as_bytes())?;
        Ok(HandlerResult::NotInterested)   // pretend to not be interested...
    }
}
