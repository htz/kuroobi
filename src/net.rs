//! Socket setup shared by the GGS clients.

use std::net::TcpStream;
use std::time::Duration;

use socket2::{SockRef, TcpKeepalive};

/// GGS drops silent connections (reported 2026-11: minutes to an hour); the OS waits two hours.
pub fn keep_alive(stream: &TcpStream) -> std::io::Result<()> {
    let ka = TcpKeepalive::new()
        .with_time(Duration::from_secs(60))
        .with_interval(Duration::from_secs(15));
    SockRef::from(stream).set_tcp_keepalive(&ka)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn keep_alive_is_on_after_setup() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        assert!(!SockRef::from(&stream).keepalive().unwrap());
        keep_alive(&stream).unwrap();
        assert!(SockRef::from(&stream).keepalive().unwrap());
    }
}
