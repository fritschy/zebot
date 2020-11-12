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
        let m = format!("{:-5}: {}\n", count, msg);
        out.write_all(m.as_bytes())?;
        Ok(HandlerResult::NotInterested)   // pretend to not be interested...
    }
}
