use nom::IResult;
use nom::lib::std::fmt::Display;

#[derive(Debug, PartialEq)]
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

#[derive(Debug, PartialEq)]
pub struct Nickname {
    nickname: String,
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

pub fn parse(mut i: &[u8]) -> IResult<&[u8], ()> {
    loop {
        dbg!(String::from_utf8_lossy(i).to_string());
        match parsers::message(i) {
            Ok((r, msg)) => {
                dbg!(msg);
                i = r;
                if i.len() == 0 {
                    break;
                }
            }

            Err(_) => {
                break;
            }
        }
    }

    Ok((i, ()))
}

mod parsers {
    use nom::{
        branch::alt,
        bytes::complete::{
            take_until,
            take_while_m_n,
        },
        character::{
            complete::{
                char, crlf, none_of,
                one_of,
            },
            is_digit,
        },
        combinator::{
            map,
            opt,
        },
        IResult,
        multi::{
            many0,
            many1,
            many_m_n,
        },
        number::complete::be_u8,
    };

    use crate::irc2::parser::parsers::utils::{string_from_parts, string_plus_char, vec2string};

    use super::*;

    mod utils {
        pub fn vec2string(v: Vec<char>) -> String {
            v.into_iter().collect()
        }

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
    pub fn message(i: &[u8]) -> IResult<&[u8], (Option<Prefix>, String, Vec<String>)> {
        let (i, prefix) = opt(parsers::prefix)(i)?;
        let (i, command) = parsers::command(i)?;
        let (i, p) = opt(params)(i)?;
        let (i, _) = crlf(i)?;
        Ok((i, (prefix, command, p.unwrap_or_else(|| Vec::new()))))
    }

    // rfc2812.txt:329
    pub fn middle(i: &[u8]) -> IResult<&[u8], String> {
        let (i, first) = nospcrlfcl(i)?;
        let (i, rest) = many0(alt((char(':'), nospcrlfcl)))(i)?;
        Ok((i, string_from_parts(first, &rest)))
    }

    // rfc2812.txt:324
    pub fn params(i: &[u8]) -> IResult<&[u8], Vec<String>> {
        map(alt((params_1, params_2)), |(mut v, x)| {
            v.push(x);
            v
        })(i)
    }

    // rfc2812.txt:324
    pub fn params_1(i: &[u8]) -> IResult<&[u8], (Vec<String>, String)> {
        fn part_1(i: &[u8]) -> IResult<&[u8], String> {
            let (i, _) = char(' ')(i)?;
            let (i, m) = middle(i)?;
            Ok((i, m))
        }
        fn part_2(i: &[u8]) -> IResult<&[u8], String> {
            let (i, _) = char(' ')(i)?;
            let (i, _) = char(':')(i)?;
            let (i, trail) = trailing(i)?;
            Ok((i, trail))
        }
        let (i, p1) = many_m_n(0, 14, part_1)(i)?;
        let (i, rest) = opt(part_2)(i)?;
        Ok((i, (p1, rest.unwrap_or_else(|| String::new()))))
    }

    // rfc2812.txt:330
    pub fn trailing(i: &[u8]) -> IResult<&[u8], String> {
        map(many0(alt((char(' '), char(':'), nospcrlfcl))), vec2string)(i)
    }

    // rfc2812.txt:325
    pub fn params_2(i: &[u8]) -> IResult<&[u8], (Vec<String>, String)> {
        let (i, _) = char(' ')(i)?;
        let (i, m) = many_m_n(14, 14, middle)(i)?;
        fn part_2(i: &[u8]) -> IResult<&[u8], String> {
            let (i, _) = char(' ')(i)?;
            let (i, _) = opt(char(':'))(i)?;
            let (i, trail) = trailing(i)?;
            Ok((i, trail))
        }
        let (i, rest) = opt(part_2)(i)?;
        Ok((i, (m, rest.unwrap_or_else(|| String::new()))))
    }

    // rfc2812.txt:327
    pub fn nospcrlfcl(i: &[u8]) -> IResult<&[u8], char> {
        none_of("\0\r\n :")(i)
    }

    // rfc2812.txt:322
    pub fn prefix(i: &[u8]) -> IResult<&[u8], Prefix> {
        let (i, _) = char(':')(i)?;
        let (i, servnick) = alt((servername, nickname_part))(i)?;
        // Note: the trailing SPACE needed to be pulled into the subparsers in order to
        //       differentiate the different parts.
        Ok((i, servnick))
    }

    // rfc2812.txt:322
    pub fn nickname_part(i: &[u8]) -> IResult<&[u8], Prefix> {
        let (i, nick) = nickname(i)?;
        fn excl_user(i: &[u8]) -> IResult<&[u8], String> {
            let (i, excl) = char('!')(i)?;
            let (i, user) = user(i)?;
            Ok((i, user))
        }
        fn at_host(i: &[u8]) -> IResult<&[u8], (Option<String>, String)> {
            let (i, u) = opt(excl_user)(i)?;
            let (i, at) = char('@')(i)?;
            let (i, h) = host(i)?;
            Ok((i, (u, h)))
        }
        let (i, rest) = opt(at_host)(i)?;
        let (i, _) = char(' ')(i)?;
        Ok((i, Prefix::Nickname(Nickname {
            nickname: nick,
            host: if let Some((_, u)) = &rest { Some(u.clone()) } else { None },
            user: if let Some((u, _)) = rest { u } else { None },
        })))
    }

    // rfc2812.txt:366
    pub fn servername(i: &[u8]) -> IResult<&[u8], Prefix> {
        let (i, s) = map(hostname, |x| Prefix::Server(x))(i)?;
        let (i, _) = char(' ')(i)?;
        Ok((i, s))
    }

    // rfc2812.txt:373
    pub fn ip4addr(i: &[u8]) -> IResult<&[u8], String> {
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

    // rfc2812.txt:374
    pub fn ip6addr(i: &[u8]) -> IResult<&[u8], String> {
        // FIXME
        let (i, x) = take_until(" ")(i)?;
        Ok((i, String::from_utf8_lossy(x).to_string()))
    }

    // rfc2812.txt:367
    pub fn host(i: &[u8]) -> IResult<&[u8], String> {
        alt((hostname, hostaddr))(i)
    }

    // rfc2812.txt:372
    pub fn hostaddr(i: &[u8]) -> IResult<&[u8], String> {
        alt((ip4addr, ip6addr))(i)
    }

    // rfc2812.txt:368
    pub fn hostname(i: &[u8]) -> IResult<&[u8], String> {
        let (i, first) = shortname(i)?;
        let (i, dot) = many0(dot_prefixed(shortname))(i)?;
        // XXX freenode services have a . at the end of ther host
        let (i, dot2) = opt(char('.'))(i)?;
        let ret = first + &dot.into_iter().collect::<String>();
        let ret = if let Some(d) = dot2 {
            ret + "."
        } else {
            ret
        };
        Ok((i, ret))
    }

    pub fn dot_prefixed<'a>(p: impl Fn(&'a [u8]) -> IResult<&'a [u8], String>) -> impl Fn(&'a [u8]) -> IResult<&'a [u8], String> {
        move |i: &[u8]| {
            // XXX we need to match / too, as these are used by freenode bots/services
            let (i, dot) = alt((char('.'), char('/')))(i)?;
            let (i, rest) = p(i)?;
            let res = String::from(dot) + rest.as_str();
            Ok((i, res))
        }
    }

    // rfc2812.txt:369
    pub fn shortname(i: &[u8]) -> IResult<&[u8], String> {
        let (i, first) = alt((letter, digit))(i)?;
        let (i, mut rest) = many0(alt((letter, digit, char('-'))))(i)?;
        let (i, mut more) = many0(alt((letter, digit)))(i)?;
        rest.append(&mut more);
        Ok((i, utils::string_from_parts(first, &rest)))
    }

    // rfc2812.txt:323
    pub fn command(i: &[u8]) -> IResult<&[u8], String> {
        let (i, cmd) = alt((
            take_while_m_n(3, 3, is_digit),
            take_until(" ")))(i)?;
        let cmd = String::from_utf8_lossy(cmd).to_string();
        Ok((i, cmd))
    }

    // rfc2812.txt:401
    pub fn user(i: &[u8]) -> IResult<&[u8], String> {
        map(many1(none_of("\0\r\n @")), |x| x.into_iter().collect::<String>())(i)
    }

    // rfc2812.txt:376
    pub fn nickname(i: &[u8]) -> IResult<&[u8], String> {
        let (i, first) = alt((letter, special))(i)?;
        // XXX the RFC specifies only up to 8 additional chars, however, e.g. freenode
        //     names may be way longer ... just go all out and use many0 to capture it all
        let (i, rest) = many0(alt((letter, digit, special, char('-'))))(i)?;
        Ok((i, utils::string_from_parts(first, &rest)))
    }

    // rfc2812.txt:407
    pub fn digit(i: &[u8]) -> IResult<&[u8], char> {
        one_of("0123456789")(i)
    }

    // rfc2812.txt:406
    pub fn letter(i: &[u8]) -> IResult<&[u8], char> {
        one_of("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ")(i)
    }

    // rfc2812.txt:409
    pub fn special(i: &[u8]) -> IResult<&[u8], char> {
        one_of("[]\\`_^{|}")(i)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn privmsg() {
        let i = b":fritschy!~fritschy@localhost PRIVMSG #zebot-test :moep\r\n";
        let i = &i[..];

        let r = super::parsers::message(i);

        assert!(r.is_ok());
        let r = r.unwrap();

        let msg = r.1;
        assert!(msg.0.is_some());

        let prefix = msg.0.unwrap();
        assert!(format!("{}", prefix) == "fritschy!~fritschy@localhost");
        assert!(msg.1 == "PRIVMSG");
        assert!(msg.2 == ["#zebot-test", "moep"]);
    }

    #[test]
    fn freenode_nickserv() {
        let i = b":NickServ!NickServ@services. NOTICE ZeBot :This nickname is registered. Please choose a different nickname, or identify via \x02/msg NickServ identify <password>\x02.\r\n";
        let i = &i[..];

        let r = super::parsers::message(i);

        assert!(r.is_ok());
        let r = r.unwrap();

        let msg = r.1;
        assert!(msg.0.is_some());

        let prefix = msg.0.unwrap();
        assert!(format!("{}", prefix) == "NickServ!NickServ@services.");
        assert!(msg.1 == "NOTICE");
        assert!(msg.2 == ["ZeBot", "This nickname is registered. Please choose a different nickname, or identify via \u{2}/msg NickServ identify <password>\u{2}."]);
    }

    #[test]
    fn freenode_bot_frigg() {
        let i = b":freenode-connect!frigg@freenode/utility-bot/frigg PRIVMSG ZeBot :\x01VERSION\x01\r\n";
        let i = &i[..];

        let r = super::parsers::message(i);

        assert!(r.is_ok());
        let r = r.unwrap();

        let msg = r.1;
        assert!(msg.0.is_some());

        let prefix = msg.0.unwrap();
        assert!(format!("{}", prefix) == "freenode-connect!frigg@freenode/utility-bot/frigg");
        assert!(msg.1 == "PRIVMSG");
        assert!(msg.2 == ["ZeBot", "\u{1}VERSION\u{1}"]);
    }

    #[test]
    fn freenode_motd_and_stuff() {
        let i = b":weber.freenode.net 372 ZeBot :- #freenode and using the \'/who freenode/staff/*\' command. You may message\r\n:weber.freenode.net 372 ZeBot :- any of us at any time. Please note that freenode predominantly provides \r\n:weber.freenode.net 372 ZeBot :- assistance via private message, and while we have a network channel the \r\n:weber.freenode.net 372 ZeBot :- primary venue for support requests is via private message to a member \r\n:weber.freenode.net 372 ZeBot :- of the volunteer staff team.\r\n:weber.freenode.net 372 ZeBot :-  \r\n:weber.freenode.net 372 ZeBot :- From time to time, volunteer staff may send server-wide notices relating to\r\n:weber.freenode.net 372 ZeBot :- the project, or the communities that we host. The majority of such notices\r\n:weber.freenode.net 372 ZeBot :- will be sent as wallops, and you can \'/mode <yournick> +w\' to ensure that you\r\n:weber.freenode.net 372 ZeBot :- do not miss them. Important messages relating to the freenode project, including\r\n:weber.freenode.net 372 ZeBot :- notices of upcoming maintenance and other scheduled downtime will be issued as\r\n:weber.freenode.net 372 ZeBot :- global notices.\r\n:weber.freenode.net 372 ZeBot :-  \r\n:weber.freenode.net 372 ZeBot :- Representing an on-topic project? Don\'t forget to register, more information\r\n:weber.freenode.net 372 ZeBot :- can be found on the https://freenode.net website under \"Group Registration\".\r\n:weber.freenode.net 372 ZeBot :-  \r\n:weber.freenode.net 372 ZeBot :- Thank you also to our server sponsors for the sustained support in keeping the\r\n:weber.freenode.net 372 ZeBot :- network going for close to two decades.\r\n:weber.freenode.net 372 ZeBot :-  \r\n:weber.freenode.net 372 ZeBot :- Thank you for using freenode!\r\n:weber.freenode.net 376 ZeBot :End of /MOTD command.\r\n:ZeBot MODE ZeBot :+i\r\n:NickServ!NickServ@services. NOTICE ZeBot :This nickname is registered. Please choose a different nickname, or identify via \x02/msg NickServ identify <password>\x02.\r\n";
        let mut i = &i[..];

        let nmsg = i.split(|&x| x == b'\r').count() - 1;

        for _ in 0..nmsg {
            let r = super::parsers::message(i);
            assert!(r.is_ok());
            i = r.unwrap().0;
        }

        assert!(super::parsers::message(i).is_err());
    }
}
