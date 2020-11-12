use smol::{
    self,
    Async,
    future::{ block_on, FutureExt, },
    io::{
        AsyncReadExt,
        AsyncWriteExt,
    },
};

mod irc;
use irc::*;

use std::net::{ ToSocketAddrs, };

async fn async_main() -> std::io::Result<()> {
    let addr = std::env::args().skip(1).next().unwrap_or_else(|| "localhost:6667".to_string())
        .to_socket_addrs()?
        .next()
        .expect("Could not resolve host address");
    let stdin = std::io::stdin();
    let mut stdin = Async::<std::io::StdinLock>::new(stdin.lock())?;
    let stdout = std::io::stdout();
    let mut stdout = Async::<std::io::StdoutLock>::new(stdout.lock())?;

    let mut stdin_buf = vec![0u8; 1024];

    let context = Context::connect(addr, User::new("ZeBot", "The Bot", None)).await?;

    context.join("#zebot-test");

    context.logon().await?;

    while !context.is_shutdown() {
        // Read from server and stdin simultaneously
        let b = async {
            stdout.write_all(b"> ").await?;
            stdout.flush().await?;
            let bytes = stdin.read(stdin_buf.as_mut_slice()).await?;

            if bytes == 0 {
                context.quit();
                return Ok(());
            }

            let bytes = &stdin_buf[..bytes];

            let x = String::from_utf8_lossy(bytes);
            let x = x.trim_end();

            context.message("#zebot-test", x);

            Ok(())
        };

        context.update().race(b).await?;
    }

    // One last update to send pending messages...
    context.update().await
}

fn main() -> std::io::Result<()> {
    block_on(async { async_main().await })
}
