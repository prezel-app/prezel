use std::net::{Ipv4Addr, SocketAddrV4};

use async_trait::async_trait;
use tokio::{
    io::{copy, split},
    net::{TcpListener, TcpStream, UnixStream},
};

use crate::listener::{Access, Listener};

pub(crate) const DOCKER_PORT: u16 = 5046;

#[derive(Clone)]
pub(crate) struct DockerBridge {
    socket: SocketAddrV4,
}

impl Default for DockerBridge {
    fn default() -> Self {
        let socket = SocketAddrV4::new(Ipv4Addr::LOCALHOST, DOCKER_PORT);
        let cloned = socket.clone();

        tokio::spawn(async move {
            let listener = TcpListener::bind(cloned).await.unwrap();
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    if let Err(e) = forward(socket).await {
                        eprintln!("failed to forward; error = {}", e);
                    }
                });
            }
        });

        Self { socket }
    }
}

#[async_trait]
impl Listener for DockerBridge {
    async fn access(&self) -> anyhow::Result<Access> {
        Ok(self.socket.into())
    }

    fn is_public(&self) -> bool {
        true
    }
}

// TODO: change to return anyhow::Result
async fn forward(inbound: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    let outbound = UnixStream::connect("/var/run/docker.sock").await?;
    let (mut ri, mut wi) = split(inbound);
    let (mut ro, mut wo) = split(outbound);

    tokio::try_join!(copy(&mut ri, &mut wo), copy(&mut ro, &mut wi))?;
    Ok(())
}
