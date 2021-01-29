use nom::{
    IResult,
    bytes::complete::{
        tag,
        take_until,
        take_while,
        take_while_m_n,
    },
    multi::{
        count,
        many_till,
    },
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

use std::fmt::{Display, Formatter};
use std::borrow::Cow;
use nom::bytes::complete::take_till1;
use nom::character::complete::{char, space1, anychar, one_of};
use nom::multi::{many0, many_m_n};

pub fn parse<'a>(i: &'a[u8]) -> IResult<&'a[u8], ()> {
    parsers::message(i)
}

mod parsers {
    use super::*;
    use futures_util::StreamExt;
    use nom::multi::many1;
    use nom::character::complete::none_of;
    use crate::irc2::parser::parsers::utils::{string_plus_char, string_from_parts};
    use nom::number::complete::be_u8;

    mod utils {
        pub fn string_from_parts(first: char, rest: &Vec<char>) -> String {
            let mut x = String::with_capacity(1 + rest.len());
            x.push(first);
            x += &rest.into_iter().collect::<String>();
            x
        }

        pub fn string_plus_char(s: String, c: char) -> String {
            let mut s = s;
            s.push(c);
            s
        }
    }

    // rfc2812.txt:321
    pub fn message<'a>(i: &'a[u8]) -> IResult<&'a[u8], ()> {
        let (i, prefix) = opt(parsers::prefix)(i)?;
        let (i, command) = parsers::command(i)?;
        let (i, p) = opt(params)(i)?;
        dbg!(prefix, command);
        Ok((i, ()))
    }

    // rfc2812.txt:329
    pub fn middle<'a>(i:&'a[u8]) -> IResult<&'a[u8], String> {
        let (i, first) = nospcrlfcl()(i)?;
        let (i, rest) = many0(alt((char(':'), nospcrlfcl())))(i)?;
        Ok((i, string_from_parts(first, &rest)))
    }

    // rfc2812.txt:324
    pub fn params<'a>(i:&'a[u8]) -> IResult<&'a[u8], String> {
        Ok((i, String::new()))
    }

    // rfc2812.txt:327
    pub fn nospcrlfcl<'a>() -> impl FnMut(&'a[u8]) -> IResult<&'a[u8], char> {
        none_of("\0\x13\x10 :")
    }

    // rfc2812.txt:322
    pub fn prefix<'a>(i:&'a[u8]) -> IResult<&'a[u8], String> {
        let (i, _) = char(':')(i)?;
        let (i, servnick) = alt((nickname_part, servername()))(i)?;
        let (i, _) = char(' ')(i)?;
        Ok((i, servnick))
    }

    pub fn nickname_part<'a>(i:&'a[u8]) -> IResult<&'a[u8], String> {
        let (i, nick) = nickname(i)?;
        let (i, excl) = opt(char('!'))(i)?;
        if let Some(excl) = excl {
            let (i, usr) = user()(i)?;
            let (i, at) = opt(char('@'))(i)?;
            let nick = string_plus_char(nick, excl) + usr.as_str();
            if let Some(at) = at {
                let (i, hst) = host()(i)?;
                return Ok((i, string_plus_char(nick, at) + hst.as_str()));
            }
            return Ok((i, nick));
        }
        Ok((i, nick))
    }

    pub fn servername<'a>() -> impl FnMut(&'a[u8]) -> IResult<&'a[u8], String> {
        alt((ip4addr, ip6addr))
    }

    pub fn ip4addr<'a>(i:&'a[u8]) -> IResult<&'a[u8], String> {
        let (i, a) = be_u8(i)?;
        let (i, _) = char('.')(i)?;
        let (i, b) = be_u8(i)?;
        let (i, _) = char('.')(i)?;
        let (i, c) = be_u8(i)?;
        let (i, _) = char('.')(i)?;
        let (i, d) = be_u8(i)?;
        // FIXME
        Ok((i, format!("{}.{}.{}.{}", a, b, c, d)))
    }

    pub fn ip6addr<'a>(i:&'a[u8]) -> IResult<&'a[u8], String> {
        // FIXME
        Ok((i, String::new()))
    }

    pub fn host<'a>() -> impl FnMut(&'a[u8]) -> IResult<&'a[u8], String> {
        alt((hostname, hostaddr))
    }

    pub fn hostaddr<'a>(i:&'a[u8]) -> IResult<&'a[u8], String> {
        Ok((i, String::new()))
    }

    pub fn hostname<'a>(i:&'a[u8]) -> IResult<&'a[u8], String> {
        let (i, first) = shortname(i)?;
        let (i, dot) = many0(dot_prefixed(shortname))(i)?;
        Ok((i, first + &dot.into_iter().collect::<String>()))
    }

    pub fn dot_prefixed<'a>(p: impl Fn(&'a[u8])->IResult<&'a[u8], String>) -> impl Fn(&'a[u8]) -> IResult<&'a[u8], String> {
        move |i:&[u8]| {
            let (i, dot) = char('.')(i)?;
            let (i, rest) = p(i)?;
            let res = String::from(dot) + rest.as_str();
            Ok((i, res))
        }
    }

    pub fn shortname<'a>(i:&'a[u8]) -> IResult<&'a[u8], String> {
        let (i, first) = alt((letter(), digit()))(i)?;
        let (i, rest) = many0(alt((letter(), digit(), char('-'))))(i)?;
        Ok((i, utils::string_from_parts(first, &rest)))
    }

    pub fn command<'a>(i:&'a[u8]) -> IResult<&'a[u8], String> {
        let (i, cmd) = alt((
            take_while_m_n(3, 3, is_digit),
            take_till1(|c| c == b' ')))(i)?;
        let (i, _) = tag(" ")(i)?;
        let cmd = String::from_utf8_lossy(cmd).to_string();
        Ok((i, cmd))
    }

    pub fn user<'a>() -> impl FnMut(&'a[u8]) -> IResult<&'a[u8], String> {
        map(many1(none_of("\0\x13\x10 @")), |x| x.into_iter().collect::<String>())
    }

    pub fn nickname<'a>(i:&'a[u8]) -> IResult<&'a[u8], String> {
        let (i, first) = alt((letter(), special()))(i)?;
        let (i, mut rest) = many_m_n(0, 8, alt((letter(), digit(), special(), char('-'))))(i)?;
        Ok((i, utils::string_from_parts(first, &rest)))
    }

    pub fn digit<'a>() -> impl Fn(&'a[u8]) -> IResult<&'a[u8], char> {
        one_of("0123456789")
    }

    pub fn letter<'a>() -> impl Fn(&'a[u8]) -> IResult<&'a[u8], char> {
        one_of("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ")
    }

    pub fn special<'a>() -> impl Fn(&'a[u8]) -> IResult<&'a[u8], char> {
        one_of("\x5b\x5c\x5d\x5e\x5f\x60\x7b\x7c\x7d[]\\`_^{|}")
    }
}
