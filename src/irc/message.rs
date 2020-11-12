use std::fmt::{
    Display,
    Formatter,
};

use nom::{
    IResult,
    bytes::complete::{
        tag,
        take_until,
        take_while,
        take_while_m_n,
    },
    multi::many_till,
    character::{
        is_alphabetic,
        is_digit,
    },
    error::Error,
    combinator::{
        opt,
        map,
        eof,
    },
    branch::alt
};

use std::borrow::Cow;
use crate::irc::*;

pub struct Message<'a> {
    pub prefix: Cow<'a, str>,
    pub command: CommandCode,
    pub params: Vec<Cow<'a, str>>,
}

impl<'a> Display for Message<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}, {}, {}",
            self.prefix,
            self.command,
            self.params.join(" "),
        )
    }
}

// This is rather messy... see rfc1459
pub(crate) fn message<'a>(i: &'a [u8]) -> IResult<&'a [u8], Message> {
    let (r, i) = take_until("\r\n")(i)?;
    let (r, _) = tag("\r\n")(r)?;

    fn prefix(i: &[u8]) -> IResult<&[u8], Cow<str>> {
        // let (i, _) = tag::<&str, &[u8], nom::error::Error<&[u8]>>(":")(i)?;
        let (i, _) = tag(":")(i)?;
        let (i, server_or_nick) = take_until(" ")(i)?;
        let (r, _) = tag(" ")(i)?;
        let server_or_nick = String::from_utf8_lossy(server_or_nick);
        Ok((r, server_or_nick))
    }

    let (i, pfx) = opt(prefix)(i)?;
    let (i, command) = alt((take_while_m_n(3, 3, is_digit), take_while(is_alphabetic)))(i)?;
    let command = String::from_utf8_lossy(command);

    fn param(i: &[u8]) -> IResult<&[u8], &[u8]> {
        if let Ok((i, _)) = tag::<_, _, Error<_>>(" ")(i) {
            if let Ok((i, _)) = tag::<_, _, Error<_>>(":")(i) {
                let (i, trailing) = take_while(|x| x != 0 && x != b'\r' && x != b'\n')(i)?;
                Ok((i, trailing))
            } else {
                take_while(|x| x != b' ' && x != 0 && x != b'\r' && x != b'\n')(i)  // middle
            }
        } else {
            Ok((i, &[]))
        }
    }

    let (_, params) = map(many_till(param, eof), |x| {
        x.0.into_iter()
            .filter(|x| !x.is_empty())
            .map(String::from_utf8_lossy)
            .collect::<Vec<_>>()
    })(i)?;

    Ok((
        r,
        Message {
            prefix: pfx.unwrap_or_default(),
            command: command.into(),
            params,
        },
    ))
}
