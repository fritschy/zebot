use crate::irc::{MessageHandler, Context, Message, HandlerResult};
use stopwatch::Stopwatch;
use crate::{is_json_flag_set, text_box};
use std::path::Path;

use tracing::error as log_error;

pub struct Callouthandler;

impl MessageHandler for Callouthandler {
    fn handle<'a>(
        &self,
        ctx: &Context,
        msg: &Message<'a>,
    ) -> Result<HandlerResult, std::io::Error> {
        if msg.params.len() < 2 || !msg.params[1].starts_with("!") {
            return Ok(HandlerResult::NotInterested);
        }

        let command = msg.params[1][1..]
            .split_ascii_whitespace()
            .next()
            .unwrap_or_else(|| "");
        if !command
            .chars()
            .all(|x| x.is_ascii_alphanumeric() || x == '-' || x == '_')
        {
            return Ok(HandlerResult::NotInterested);
        }

        let command = command.to_lowercase();

        let path = format!("./handlers/{}", command);
        let path = Path::new(&path);

        if !path.exists() {
            return Ok(HandlerResult::NotInterested);
        }

        let nick = msg.get_nick();
        let mut args = msg.params.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        args.insert(0, nick); // this sucks

        // Handler args look like this:
        // $srcnick $src(chan,query) "!command[ ...args]"

        // json from handler
        // { "lines": [ ... ],
        //   "dst": "nick" | "channel",   # optional
        //   "box": "0"|"1"|true|false,   # optional
        //   "wrap": "0"|"1"              # optional
        //   "wrap_single_lines": "0"|"1" # optional
        //   "title": "string"            # optional
        //   "link": "string"             # optional
        // }

        dbg!(&args);

        let s = Stopwatch::start_new();
        let cmd = std::process::Command::new(path).args(&args).output();
        let s = s.elapsed();

        log_error!("Handler {} completed in {:?}", command, s);

        match cmd {
            Ok(p) => {
                if !p.status.success() {
                    let dst = msg.get_reponse_destination(&ctx.joined_channels.borrow());
                    log_error!("Handler failed with code {}", p.status.code().unwrap());
                    dbg!(&p);
                    ctx.message(&dst, "Somehow, that did not work...");
                    return Ok(HandlerResult::Handled);
                }

                if let Ok(response) = String::from_utf8(p.stdout) {
                    dbg!(&response);
                    match json::parse(&response) {
                        Ok(response) => {
                            let dst = if response.contains("dst") {
                                response["dst"].to_string()
                            } else {
                                msg.get_reponse_destination(&ctx.joined_channels.borrow())
                            };

                            if response.contains("error") {
                                dbg!(&response);
                                ctx.message(&dst, "Somehow, that did not work...");
                                return Ok(HandlerResult::Handled);
                            } else {
                                if !is_json_flag_set(&response["box"]) {
                                    for l in response["lines"].members() {
                                        ctx.message(&dst, &l.to_string());
                                    }
                                } else {
                                    let lines = response["lines"]
                                        .members()
                                        .map(|x| x.to_string())
                                        .collect::<Vec<_>>();
                                    let lines = if is_json_flag_set(&response["wrap"])
                                        && lines.iter().map(|x| x.len()).any(|l| l > 80)
                                    {
                                        let nlines = lines.len();

                                        let s = if lines[nlines - 1].starts_with("    ") {
                                            let (lines, last) = lines.split_at(nlines - 1);

                                            let s = lines.concat();
                                            let s = textwrap::fill(&s, 80);

                                            let s = s + "\n";
                                            s + last[0].as_str()
                                        } else {
                                            let s = lines.concat();
                                            textwrap::fill(&s, 80)
                                        };

                                        s.split(|f| f == '\n')
                                            .map(|x| x.to_string())
                                            .collect::<Vec<_>>()
                                    } else if is_json_flag_set(&response["wrap_single_lines"]) {
                                        let mut new_lines = Vec::with_capacity(lines.len());
                                        let opt = textwrap::Options::new(80)
                                            .splitter(textwrap::NoHyphenation)
                                            .subsequent_indent("  ");
                                        for l in lines {
                                            new_lines.extend(
                                                textwrap::wrap(&l, &opt)
                                                    .iter()
                                                    .map(|x| x.to_string()),
                                            );
                                        }
                                        new_lines
                                    } else {
                                        lines
                                    };

                                    // append link if provided
                                    let lines = if let Some(s) = response["link"].as_str() {
                                        let mut lines = lines;
                                        lines.push(format!("    -- {}", s));
                                        lines
                                    } else {
                                        lines
                                    };

                                    for i in text_box(lines.iter(), response["title"].as_str()) {
                                        ctx.message(&dst, &i);
                                    }
                                }
                            }
                        }

                        Err(e) => {
                            // Perhaps have this as a fallback for non-json handlers? What could possibly go wrong!
                            log_error!(
                                "Could not parse json from handler {}: {}",
                                command, response
                            );
                            log_error!("Error: {:?}", e);
                        }
                    }
                } else {
                    log_error!("Could not from_utf8 for handler {}", command);
                }
            }

            Err(e) => {
                log_error!("Could not execute handler: {:?}", e);
                return Ok(HandlerResult::NotInterested);
            }
        }

        Ok(HandlerResult::Handled)
    }
}
