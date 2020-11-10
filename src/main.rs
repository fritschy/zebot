use async_std::{self as astd, io, net::{TcpStream, ToSocketAddrs}, task, prelude::*};

mod irc;
mod handler;

enum StreamID<T> {
    Stdin(T),
    Server(T),
}

async fn async_main(handler: &mut handler::MessageHandler) -> std::io::Result<()> {
    let addr = "irc.freenode.net:6667"
        .to_socket_addrs()
        .await?
        .next()
        .expect("Could not resolve host address");
    let mut connection = TcpStream::connect(addr).await?;
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();

    let mut stdin_buf = vec![0u8; 1024];
    let mut serve_buf = vec![0u8; 1024];

    // will contain remains of he last read that could not be parsed as a message...
    let mut remaining_buf = Vec::new();

    let mut count: usize = 0;
    loop {
        // Read from server and stdin simultaneously
        let bytes = {
            let a = async {
                // Need to complete a previous message
                let off = if !remaining_buf.is_empty() {
                    &mut serve_buf[..remaining_buf.len()].copy_from_slice(remaining_buf.as_slice());
                    let off = remaining_buf.len();
                    remaining_buf.clear();
                    off
                } else {
                    0
                };

                let bytes = connection
                    .read(&mut serve_buf.as_mut_slice()[off..])
                    .await?;

                Ok::<_, std::io::Error>(StreamID::Server(&serve_buf[..off + bytes]))
            };

            let b = async {
                stdout.write_all(b"> ").await?;
                stdout.flush().await?;
                let bytes = stdin.read(stdin_buf.as_mut_slice()).await?;
                Ok::<_, std::io::Error>(StreamID::Stdin(&stdin_buf[..bytes]))
            };

            a.race(b).await
        }?;

        match bytes {
            StreamID::Server(buf) => {
                let mut i = buf;
                loop {
                    match irc::message(i) {
                        Ok((r, msg)) => {
                            i = r;
                            count += 1;

                            handler.handle(&mut connection, count, &msg)?;
                        }

                        Err(_) => {
                            remaining_buf.reserve(i.len());
                            for x in i {
                                remaining_buf.push(*x);
                            }
                            break;
                        }
                    }
                }
            }
            StreamID::Stdin(buf) => {
                if buf.is_empty() {
                    connection.write_all(b"QUIT\r\n").await?;
                    connection.shutdown(astd::net::Shutdown::Both)?;
                    break;
                }

                let x = String::from_utf8_lossy(buf);
                let x = x.trim_end();
                println!("Got from stdin: {}", x);
            }
        }
    }

    Ok(())
}

fn main() -> std::io::Result<()> {
    let mut msg_handler =
        handler::MessageHandler::with_nick("ZeBot").channel(handler::Channel::Name("#zebot-test".to_string()));

    task::block_on(async { async_main(&mut msg_handler).await })
}
