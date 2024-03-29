use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use bytes::Bytes;
use h2::{client, SendStream};
use h2::client::{ResponseFuture, SendRequest};
use http::Request;
use reqwest::Url;
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use crate::tls;

#[derive(Clone)]
pub struct Connection {
    send_request: SendRequest<Bytes>,
    status_sender: Sender<bool>,
    requests_counter: Arc<AtomicU32>,
}

impl Connection {
    pub async fn new(base_url: &Url) -> (Connection, Receiver<bool>) {
        let send_request = connect_to_server(base_url).await;
        let (sender, receiver) = mpsc::channel(1);

        (
            Connection {
                send_request,
                status_sender: sender,
                requests_counter: Arc::new(AtomicU32::new(0)),
            },
            receiver
        )
    }

    pub async fn ready(self) -> Result<SendRequest<Bytes>, h2::Error> {
        self.send_request.ready().await
    }

    pub fn send_request(&mut self, request: Request<()>) -> Result<(ResponseFuture, SendStream<Bytes>), h2::Error> {
        self.requests_counter.fetch_add(1, Ordering::Relaxed);
        self.send_request.send_request(request, true)
    }

    pub fn requests_sent(& self) -> u32 {
        self.requests_counter.load(Ordering::Relaxed)
    }

    pub async fn mark_connection_unhealthy(&self) {
        match self.status_sender.try_send(false) {
            Ok(_) => {
                println!{"reported on unhealthy connection"}
            }
            Err(err) => {
                eprintln!("error reporting unhealthy connection: {}", err)
            }
        }
    }
}
async fn connect_to_server(url: &Url) -> SendRequest<Bytes> {
    let host = url.host_str().expect("URL does not contain a host");
    let port = url.port().unwrap_or(443);

    loop {
        match attempt_connect(host, port).await {
            Ok(v) => return v,
            Err(err) => {
                eprintln!("{}", err);
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        }
    }
}

async fn attempt_connect(host: &str, port: u16) -> Result<SendRequest<Bytes>, Box<dyn Error + Send>> {
    // // Load the system's root certificates
    // let mut root_store = RootCertStore::empty();
    // root_store.add_parsable_certificates(
    //     &rustls_native_certs::load_native_certs().expect("failed to load native certs")
    // );
    //
    // let mut config = ClientConfig::builder()
    //     .with_safe_defaults()
    //     // .with_custom_certificate_verifier(Arc::new(tls::NoCertificateVerification{}))
    //     .with_root_certificates(root_store)
    //     .with_no_client_auth();

    let mut config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(tls::SkipServerVerification{}))
        .with_no_client_auth();

    config.alpn_protocols.push(b"h2".to_vec()); // Enable HTTP/2 ALPN protocol negotiation.
    let tls_connector = TlsConnector::from(Arc::new(config));

    let authority = format!("{}:{}", host, port);
    let tcp_stream = match TcpStream::connect(&authority).await {
        Ok(v) => v,
        Err(err) => return Err(Box::new(err))
    };
    let host_arc: String = host.to_owned();
    let tls_stream = match tls_connector
        .connect(
            ServerName::try_from(host_arc)
                .expect(&format!("invalid host name: {}", host)),
            tcp_stream
        ).await {
        Ok(v) => v,
        Err(err) => return Err(Box::new(err))
    };
    return match negotiate_h2_connection(tls_stream).await {
        Ok(v) => Ok(v),
        Err(err) => Err(Box::new(err))
    };
}

async fn negotiate_h2_connection(tcp: TlsStream<TcpStream>) -> Result<SendRequest<Bytes>, h2::Error> {
    let (send_request, connection) = client::handshake(tcp).await?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            eprintln!("Error occurred while waiting for connection to close: {}", err);
        }
    });
    Ok(send_request)
}
