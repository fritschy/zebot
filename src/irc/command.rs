use std::borrow::Cow;
use std::fmt::{Display, Formatter};

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
    Error,
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
                b"ERROR" => CommandCode::Error,
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
            CommandCode::Error => write!(f, "ERROR")?,
            CommandCode::Unknown => write!(f, "UNKNOWN")?,
            CommandCode::Numeric(n) => write!(f, "{:03}", n)?,
            CommandCode::Generic(n) => write!(f, "{}", n)?,
        }
        Ok(())
    }
}
