use crate::irc::*;
use irc2::Message;

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
    fn handle(&self, ctx: &Context, msg: &Message) -> Result<HandlerResult, std::io::Error> {
        // Freenode sends a prefix, ircdng does not, but has a param we can use.
        let dst = if let Some(prefix) = &msg.prefix {
            prefix.to_string()
        } else if !msg.params.is_empty() {
            msg.params[0].clone()
        } else {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "Don't know how to respond to PING w/o params or prefix!"));
        };

        let resp = format!("PONG {} :{}\r\n", dst, dst);

        ctx.send(resp);

        Ok(HandlerResult::Handled)
    }
}
