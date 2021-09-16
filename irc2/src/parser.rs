use nom::IResult;
use tracing::info;

use crate::*;

pub fn parse(i: &[u8]) -> IResult<&[u8], Message> {
    loop {
        match parsers::message(i) {
            Ok((r, msg)) => {
                info!("{:4}", msg);
                return Ok((r, msg));
            }

            Err(_) => {
                break;
            }
        }
    }

    Err(nom::Err::Error(nom::error::Error::new(i,nom::error::ErrorKind::IsNot)))
}

mod parsers {
    use nom::{
        branch::alt,
        bytes::complete::{take_until, take_while, take_while1, take_while_m_n},
        character::{
            complete::{char, crlf, none_of, one_of},
            is_alphabetic, is_digit,
        },
        combinator::{map, opt, recognize},
        IResult,
        multi::{many0, many_m_n},
        number::complete::be_u8,
    };

    use super::*;

    // rfc2812.txt:321
    pub(crate) fn message(i: &[u8]) -> IResult<&[u8], Message> {
        let (i, prefix) = opt(parsers::prefix)(i)?;
        let (i, command) = parsers::command(i)?;
        let (i, p) = opt(params)(i)?;
        let (i, _) = crlf(i)?;
        let p = p.map(|x| x.iter().map(|x| String::from_utf8_lossy(*x).to_string()).collect());
        Ok((
            i,
            Message {
                prefix,
                command: command.into(),
                params: p.unwrap_or_else(|| Vec::new()),
            },
        ))
    }

    // rfc2812.txt:329
    fn middle(i: &[u8]) -> IResult<&[u8], &[u8]> {
        pub fn middle_(i: &[u8]) -> IResult<&[u8], &[u8]> {
            let (i, _first) = nospcrlfcl(i)?;
            take_while(|c| c == b':' || is_nospcrlfcl(c))(i)
        }
        recognize(middle_)(i)
    }

    // rfc2812.txt:324
    fn params(i: &[u8]) -> IResult<&[u8], Vec<&[u8]>> {
        // rfc2812.txt:324
        pub fn params_(i: &[u8]) -> IResult<&[u8], (Vec<&[u8]>, &[u8])> {
            fn part_1(i: &[u8]) -> IResult<&[u8], &[u8]> {
                let (i, _) = char(' ')(i)?;
                let (i, m) = middle(i)?;
                Ok((i, m))
            }
            fn part_2(i: &[u8]) -> IResult<&[u8], &[u8]> {
                let (i, _) = char(' ')(i)?;
                let (i, _) = opt(char(':'))(i)?;
                let (i, trail) = trailing(i)?;
                Ok((i, trail))
            }
            let (i, p1) = many_m_n(0, 14, part_1)(i)?;
            let (i, rest) = opt(part_2)(i)?;
            Ok((i, (p1, rest.unwrap_or_else(|| &[]))))
        }

        map(params_, |(mut v, x)| {
            v.push(x);
            v
        })(i)
    }

    // rfc2812.txt:330
    fn trailing(i: &[u8]) -> IResult<&[u8], &[u8]> {
        take_while(|c| c == b' ' || c == b':' || is_nospcrlfcl(c))(i)
    }

    // rfc2812.txt:327
    fn nospcrlfcl(i: &[u8]) -> IResult<&[u8], char> {
        none_of("\0\r\n :")(i)
    }

    fn is_nospcrlfcl(c: u8) -> bool {
        c != 0 && c != b'\r' && c != b'\n' && c != b' ' && c != b':'
    }

    // rfc2812.txt:322
    fn prefix(i: &[u8]) -> IResult<&[u8], Prefix> {
        let (i, _) = char(':')(i)?;
        let (i, servnick) = alt((prefix_servername, prefix_nickname))(i)?;
        // Note: the trailing SPACE needed to be pulled into the subparsers in order to
        //       differentiate the parts.
        Ok((i, servnick))
    }

    // rfc2812.txt:322
    fn prefix_nickname(i: &[u8]) -> IResult<&[u8], Prefix> {
        let (i, nick) = nickname(i)?;
        fn excl_user(i: &[u8]) -> IResult<&[u8], &[u8]> {
            let (i, _excl) = char('!')(i)?;
            let (i, user) = user(i)?;
            Ok((i, user))
        }
        fn at_host(i: &[u8]) -> IResult<&[u8], (Option<&[u8]>, &[u8])> {
            let (i, u) = opt(excl_user)(i)?;
            let (i, _at) = char('@')(i)?;
            let (i, h) = host(i)?;
            Ok((i, (u, h)))
        }
        let (i, rest) = opt(at_host)(i)?;
        let (i, _) = char(' ')(i)?;
        Ok((
            i,
            Prefix::Nickname(Nickname {
                nickname: String::from_utf8_lossy(nick).to_string(),
                host: if let Some((_, u)) = &rest {
                    Some(String::from_utf8_lossy(u).to_string())
                } else {
                    None
                },
                user: if let Some((u, _)) = rest { u.map(|u| String::from_utf8_lossy(u).to_string()) } else { None },
            }),
        ))
    }

    // rfc2812.txt:366
    fn prefix_servername(i: &[u8]) -> IResult<&[u8], Prefix> {
        let (i, s) = map(hostname, |x| Prefix::Server(String::from_utf8_lossy(x).to_string()))(i)?;
        let (i, _) = char(' ')(i)?;
        Ok((i, s))
    }

    // rfc2812.txt:373
    fn ip4addr(i: &[u8]) -> IResult<&[u8], &[u8]> {
        pub fn ip(i: &[u8]) -> IResult<&[u8], u8> {
            let (i, _) = take_while_m_n(1, 3, is_digit)(i)?;
            let (i, _) = char('.')(i)?;
            let (i, _) = take_while_m_n(1, 3, is_digit)(i)?;
            let (i, _) = char('.')(i)?;
            let (i, _) = take_while_m_n(1, 3, is_digit)(i)?;
            let (i, _) = char('.')(i)?;
            let (i, _) = take_while_m_n(1, 3, is_digit)(i)?;
            be_u8(i)
        }
        recognize(ip)(i)
    }

    // rfc2812.txt:374
    fn ip6addr(i: &[u8]) -> IResult<&[u8], &[u8]> {
        // FIXME
        let (i, x) = take_until(" ")(i)?;
        Ok((i, x))
    }

    // rfc2812.txt:367
    fn host(i: &[u8]) -> IResult<&[u8], &[u8]> {
        alt((hostname, hostaddr))(i)
    }

    // rfc2812.txt:372
    fn hostaddr(i: &[u8]) -> IResult<&[u8], &[u8]> {
        alt((ip4addr, ip6addr))(i)
    }

    // rfc2812.txt:368
    pub fn hostname(i: &[u8]) -> IResult<&[u8], &[u8]> {
        pub fn hostname_(i: &[u8]) -> IResult<&[u8], &[u8]> {
            let (i, _first) = shortname(i)?;
            let (i, _dot) = many0(dot_prefixed(shortname))(i)?;
            // XXX freenode services have a . at the end of their host
            let (i, _dot2) = opt(char('.'))(i)?;
            Ok((i, i))
        }
        recognize(hostname_)(i)
    }

    fn dot_prefixed<'a>(
        p: impl Fn(&'a [u8]) -> IResult<&'a [u8], &'a [u8]>,
    ) -> impl Fn(&'a [u8]) -> IResult<&'a [u8], &'a [u8]> {
        move |i: &'a [u8]| {
            recognize(|i: &'a [u8]| {
                // XXX we need to match / too, as these are used by freenode bots/services
                let (i, _dot) = alt((char('.'), char('/')))(i)?;
                let (i, _rest) = p(i)?;
                Ok((i, i))
            })(i)
        }
    }

    // rfc2812.txt:369
    fn shortname(i: &[u8]) -> IResult<&[u8], &[u8]> {
        pub fn shortname_(i: &[u8]) -> IResult<&[u8], &[u8]> {
            let (i, _first) = alt((letter, digit))(i)?;
            let (i, mut _rest) = take_while(|c| is_alphabetic(c) || is_digit(c) || c == b'-')(i)?;
            let (i, mut _more) = take_while(|c| is_alphabetic(c) || is_digit(c))(i)?;
            Ok((i, i))
        }
        recognize(shortname_)(i)
    }

    // rfc2812.txt:323
    fn command(i: &[u8]) -> IResult<&[u8], &[u8]> {
        let (i, cmd) = alt((take_while_m_n(3, 3, is_digit), take_until(" ")))(i)?;
        Ok((i, cmd))
    }

    // rfc2812.txt:401
    fn user(i: &[u8]) -> IResult<&[u8], &[u8]> {
        take_while1(|c: u8| !(b"\0\r\n @".contains(&c)))(i)
    }

    // rfc2812.txt:376
    fn nickname(i: &[u8]) -> IResult<&[u8], &[u8]> {
        fn nickname_(i: &[u8]) -> IResult<&[u8], &[u8]> {
            let (i, _first) = alt((letter, special))(i)?;
            // XXX the RFC specifies only up to 8 additional chars, however, e.g. freenode
            //     names may be way longer ... just go all out and use many0 to capture it all
            let (i, _rest) =
                take_while(|c| is_alphabetic(c) || is_digit(c) || is_special(c) || c == b'-')(i)?;
            Ok((i, i))
        }
        recognize(nickname_)(i)
    }

    // rfc2812.txt:407
    fn digit(i: &[u8]) -> IResult<&[u8], char> {
        one_of("0123456789")(i)
    }

    // rfc2812.txt:406
    fn letter(i: &[u8]) -> IResult<&[u8], char> {
        one_of("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ")(i)
    }

    // rfc2812.txt:409
    fn special(i: &[u8]) -> IResult<&[u8], char> {
        one_of("[]\\`_^{|}")(i)
    }

    // rfc2812.txt:409
    fn is_special(i: u8) -> bool {
        b"[]\\`_^{|}".contains(&i)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn privmsg() {
        let i = &b":fritschy!~fritschy@localhost PRIVMSG #zebot-test :moep\r\n"[..];

        let r = super::parsers::message(i);

        assert!(r.is_ok());
        let r = r.unwrap();

        let msg = r.1;
        assert!(msg.prefix.is_some());

        let prefix = msg.prefix.unwrap();
        assert!(format!("{}", prefix) == "fritschy!~fritschy@localhost");
        assert!(msg.command == (&b"PRIVMSG"[..]).into());
        assert!(msg.params == [&"#zebot-test"[..], &"moep"[..]]);
    }

    #[test]
    fn freenode_nickserv() {
        let i = &b":NickServ!NickServ@services. NOTICE ZeBot :This nickname is registered. Please choose a different nickname, or identify via \x02/msg NickServ identify <password>\x02.\r\n"[..];

        let r = super::parsers::message(i);

        assert!(r.is_ok());
        let r = r.unwrap();

        let msg = r.1;
        assert!(msg.prefix.is_some());

        let prefix = msg.prefix.unwrap();
        assert!(format!("{}", prefix) == "NickServ!NickServ@services.");
        assert!(msg.command == (&b"NOTICE"[..]).into());
        assert!(msg.params == [&"ZeBot"[..], &"This nickname is registered. Please choose a different nickname, or identify via \x02/msg NickServ identify <password>\x02."[..]]);
    }

    #[test]
    fn freenode_bot_frigg() {
        let i = &b":freenode-connect!frigg@freenode/utility-bot/frigg PRIVMSG ZeBot :\x01VERSION\x01\r\n"[..];

        let r = super::parsers::message(i);

        assert!(r.is_ok());
        let r = r.unwrap();

        let msg = r.1;
        assert!(msg.prefix.is_some());

        let prefix = msg.prefix.unwrap();
        assert!(format!("{}", prefix) == "freenode-connect!frigg@freenode/utility-bot/frigg");
        assert!(msg.command == (&b"PRIVMSG"[..]).into());
        assert!(msg.params == [&"ZeBot"[..], &"\x01VERSION\x01"[..]]);
    }

    #[test]
    fn freenode_motd_and_stuff() {
        let mut i = &b":weber.freenode.net 372 ZeBot :- #freenode and using the \'/who freenode/staff/*\' command. You may message\r\n:weber.freenode.net 372 ZeBot :- any of us at any time. Please note that freenode predominantly provides \r\n:weber.freenode.net 372 ZeBot :- assistance via private message, and while we have a network channel the \r\n:weber.freenode.net 372 ZeBot :- primary venue for support requests is via private message to a member \r\n:weber.freenode.net 372 ZeBot :- of the volunteer staff team.\r\n:weber.freenode.net 372 ZeBot :-  \r\n:weber.freenode.net 372 ZeBot :- From time to time, volunteer staff may send server-wide notices relating to\r\n:weber.freenode.net 372 ZeBot :- the project, or the communities that we host. The majority of such notices\r\n:weber.freenode.net 372 ZeBot :- will be sent as wallops, and you can \'/mode <yournick> +w\' to ensure that you\r\n:weber.freenode.net 372 ZeBot :- do not miss them. Important messages relating to the freenode project, including\r\n:weber.freenode.net 372 ZeBot :- notices of upcoming maintenance and other scheduled downtime will be issued as\r\n:weber.freenode.net 372 ZeBot :- global notices.\r\n:weber.freenode.net 372 ZeBot :-  \r\n:weber.freenode.net 372 ZeBot :- Representing an on-topic project? Don\'t forget to register, more information\r\n:weber.freenode.net 372 ZeBot :- can be found on the https://freenode.net website under \"Group Registration\".\r\n:weber.freenode.net 372 ZeBot :-  \r\n:weber.freenode.net 372 ZeBot :- Thank you also to our server sponsors for the sustained support in keeping the\r\n:weber.freenode.net 372 ZeBot :- network going for close to two decades.\r\n:weber.freenode.net 372 ZeBot :-  \r\n:weber.freenode.net 372 ZeBot :- Thank you for using freenode!\r\n:weber.freenode.net 376 ZeBot :End of /MOTD command.\r\n:ZeBot MODE ZeBot :+i\r\n:NickServ!NickServ@services. NOTICE ZeBot :This nickname is registered. Please choose a different nickname, or identify via \x02/msg NickServ identify <password>\x02.\r\n"[..];

        let nmsg = i.split(|&x| x == b'\r').count() - 1;

        for _ in 0..nmsg {
            let r = super::parsers::message(i);
            assert!(r.is_ok());
            i = r.unwrap().0;
        }

        assert!(super::parsers::message(i).is_err());
    }
}
