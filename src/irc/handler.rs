use smol::{
    io::AsyncWriteExt,
    future::block_on,
};

#[derive(Clone, PartialEq)]
pub(crate) enum Channel {
    Name(String),
}

pub(crate) struct MessageHandler {
    pub(crate) nick: String,
    pub(crate) channels: Vec<Channel>,
}

impl MessageHandler {
    pub(crate) fn with_nick(n: &str) -> Self {
        MessageHandler {
            nick: n.to_string(),
            channels: Vec::new(),
        }
    }

    pub(crate) fn channel(mut self, c: Channel) -> Self {
        self.channels.push(c);
        self
    }

    pub(crate) fn handle(
        &mut self,
        ret: &mut smol::Async<std::net::TcpStream>,
        id: usize,
        msg: &crate::irc::Message,
    ) -> std::io::Result<()> {
        println!(
            "{}: {} {} {:?}",
            id,
            msg.prefix,
            msg.command,
            msg.params
        );
        match msg.command {
            crate::irc::CommandCode::Ping => {
                let response = format!("PONG {}\r\n", msg.params[0]);
                println!("Sending: {}", response);
                block_on(async { ret.write_all(response.as_bytes()).await })
            }

            _ => {
                if id == 1 {
                    // First message: "logon"
                    block_on(async {
                        let msg = format!(
                            "USER {} none none :The Bot\r\nNICK {}\r\n{}",
                            self.nick,
                            self.nick,
                            self.channels.iter().fold(String::new(), |acc, x| {
                                match x {
                                    Channel::Name(n) => format!("{}JOIN {}\r\n", acc, n),
                                }
                            })
                        );
                        println!("Sending: {}", msg);
                        ret.write_all(msg.as_bytes()).await
                    })
                } else {
                    Ok(())
                }
            }
        }
    }
}
