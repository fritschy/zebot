use nom::lib::std::fmt::Display;

mod parser;

#[derive(Debug, PartialEq)]
pub enum Prefix<'a> {
    Server(&'a [u8]),
    Nickname(Nickname<'a>),
}

impl<'a> Display for Prefix<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        match self {
            Prefix::Server(s) => write!(f, "{}", String::from_utf8_lossy(s)),
            Prefix::Nickname(n) => write!(f, "{}", n),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Nickname<'a> {
    nickname: &'a [u8],
    // XXX: in rfc2812 this should actually be an host: Option<(Option<user>, host)>
    //      but I really dont want to be it this way...
    user: Option<&'a [u8]>,
    host: Option<&'a [u8]>,
}

impl<'a> Display for Nickname<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        write!(f, "{}", String::from_utf8_lossy(self.nickname))?;
        if let Some(host) = &self.host {
            if let Some(user) = &self.user {
                write!(f, "!{}", String::from_utf8_lossy(user))?;
            }
            write!(f, "@{}", String::from_utf8_lossy(host))?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
pub struct Message<'a> {
    prefix: Option<Prefix<'a>>,
    command: &'a [u8],
    params: Vec<&'a [u8]>,
}

impl<'a> Display for Message<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        if let Some(p) = &self.prefix {
            write!(f, "P:{} ", p)?;
        }
        write!(f, "C:{} ", String::from_utf8_lossy(self.command))?;
        if !self.params.is_empty() {
            for p in &self.params {
                write!(f, "'{}' ", String::from_utf8_lossy(p))?;
            }
        }
        Ok(())
    }
}

pub fn parse_ng<'a>(i: &'a [u8]) {
    parser::parse(i);
}
