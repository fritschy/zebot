use nom::lib::std::fmt::Display;

use tracing::error as log_error;

mod parser;
pub mod command;

pub use parser::parse;

#[derive(Debug, PartialEq, Clone)]
pub enum Prefix {
    Server(String),
    Nickname(Nickname),
}

impl Display for Prefix {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        match self {
            Prefix::Server(s) => write!(f, "{}", s),
            Prefix::Nickname(n) => write!(f, "{}", n),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct Nickname {
    nickname: String,
    // XXX: in rfc2812 this should actually be an host: Option<(Option<user>, host)>
    //      but I really dont want to be it this way...
    user: Option<String>,
    host: Option<String>,
}

impl Display for Nickname {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        write!(f, "{}", self.nickname)?;
        if let Some(host) = &self.host {
            if let Some(user) = &self.user {
                write!(f, "!{}", user)?;
            }
            write!(f, "@{}", host)?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct Message {
    pub prefix: Option<Prefix>,
    pub command: command::CommandCode,
    pub params: Vec<String>,
}

impl Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        if let Some(p) = &self.prefix {
            write!(f, "P:{} ", p)?;
        }
        write!(f, "C:{} ", self.command.to_string())?;
        if !self.params.is_empty() {
            for p in &self.params {
                write!(f, "'{}' ", p)?;
            }
        }
        Ok(())
    }
}

impl Message {
    pub fn get_reponse_destination(&self, channels: &[String]) -> String {
        if channels.iter().any(|x| x == &self.params[0]) {
            self.params[0].clone()
        } else {
            self.get_nick()
        }
    }

    pub fn get_nick(&self) -> String {
        if let Some(Prefix::Nickname(Nickname{nickname, ..})) = &self.prefix {
            nickname.clone()
        } else {
            String::new() // FIXME: WAH!
        }
    }
}
