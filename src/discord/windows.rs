use super::IO_TIMEOUT;
use std::{io, path::Path};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{ClientOptions, NamedPipeClient},
    runtime::{Builder, Runtime},
    time::timeout,
};

pub(super) struct Pipe {
    client: NamedPipeClient,
    runtime: Runtime,
    first_byte: Option<u8>,
}

impl Pipe {
    pub(super) fn connect(path: &Path) -> io::Result<Self> {
        let runtime = Builder::new_current_thread().enable_all().build()?;
        let client = {
            let _guard = runtime.enter();
            ClientOptions::new().open(path)?
        };
        Ok(Self {
            client,
            runtime,
            first_byte: None,
        })
    }

    pub(super) fn has_data(&mut self) -> io::Result<bool> {
        let mut byte = [0];
        match self.runtime.block_on(async {
            // Drive the I/O reactor before checking its readiness, even when it
            // has not been polled since the previous Discord acknowledgement.
            timeout(std::time::Duration::ZERO, self.client.read(&mut byte)).await
        }) {
            Ok(Ok(0)) => Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(Ok(_)) => {
                self.first_byte = Some(byte[0]);
                Ok(true)
            }
            Ok(Err(error)) => Err(error),
            Err(_) => Ok(false),
        }
    }
}

impl io::Read for Pipe {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if let Some(byte) = self.first_byte.take() {
            buffer[0] = byte;
            return Ok(1);
        }
        self.runtime
            .block_on(async { timeout(IO_TIMEOUT, self.client.read(buffer)).await })?
    }
}

impl io::Write for Pipe {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.runtime
            .block_on(async { timeout(IO_TIMEOUT, self.client.write(buffer)).await })?
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        time::Instant,
    };
    use tokio::net::windows::named_pipe::ServerOptions;

    #[test]
    fn polls_and_reads_discord_ipc_over_a_named_pipe() {
        let path = format!(r"\\.\pipe\t-ui-uxplay-test-{}", std::process::id());
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let mut server = {
            let _guard = runtime.enter();
            ServerOptions::new().create(&path).unwrap()
        };
        let mut client = Pipe::connect(Path::new(&path)).unwrap();
        assert!(!client.has_data().unwrap());
        runtime.block_on(async {
            timeout(IO_TIMEOUT, server.connect())
                .await
                .unwrap()
                .unwrap();
            timeout(IO_TIMEOUT, server.write_all(b"ping"))
                .await
                .unwrap()
                .unwrap();
        });

        let deadline = Instant::now() + IO_TIMEOUT;
        while !client.has_data().unwrap() {
            assert!(
                Instant::now() < deadline,
                "Discord pipe never became readable"
            );
        }
        let mut buffer = [0; 4];
        client.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer, b"ping");
        client.write_all(b"pong").unwrap();
        runtime.block_on(async {
            timeout(IO_TIMEOUT, server.read_exact(&mut buffer))
                .await
                .unwrap()
                .unwrap();
        });
        assert_eq!(&buffer, b"pong");
    }
}
