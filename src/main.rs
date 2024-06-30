use std::collections::{HashMap};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::{thread};
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;
use uuid::Uuid;

use codepage_437::{CP437_CONTROL, FromCp437};
use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use telnet::{Telnet, Event as TelnetEvent, TelnetOption, Action, TelnetError};
use local_ip_address::local_ip;

pub enum ClientManagerMessage {
    Connect {
        stream: TcpStream
    },
    ConnectionClosed {
        client_id: Uuid
    },
}

#[derive(Clone)]
pub struct ClientConnection {
    client_id: Uuid,
    ip_addr: IpAddr,
}

#[derive(Clone)]
pub struct SharedClientMap {
    inner: Arc<Mutex<SharedMapInner>>,
}

struct SharedMapInner {
    data: HashMap<uuid::Uuid, ClientConnection>,
}

impl SharedClientMap {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SharedMapInner {
                data: HashMap::new(),
            }))
        }
    }

    pub fn insert(&self, key: uuid::Uuid, value: ClientConnection) {
        let mut lock = self.inner.lock().unwrap();
        lock.data.insert(key, value);
    }

    pub fn get(&self, key: uuid::Uuid) -> Option<ClientConnection> {
        let lock = self.inner.lock().unwrap();
        lock.data.get(&key).cloned()
    }

    pub fn remove(&self, key: uuid::Uuid) {
        let mut lock = self.inner.lock().unwrap();
        let _ = lock.data.remove(&key);
    }

    pub fn len(&self) -> usize {
        let lock = self.inner.lock().unwrap();
        lock.data.len()
    }
}


#[derive(Clone)]
pub struct ClientManager {
    receiver: Receiver<ClientManagerMessage>,
    clients: SharedClientMap,
}

impl ClientManager {
    pub fn new(receiver: Receiver<ClientManagerMessage>) -> Self {
        Self {
            receiver,
            clients: SharedClientMap::new(),
        }
    }

    pub fn receive(&self) -> Result<ClientManagerMessage, TryRecvError> {
        return self.receiver.try_recv();
    }
}

fn main() {
    let tcp_listener = start_telnet_server();
    let (client_manager_tx, client_manager_rx) = unbounded();
    launch_client_manager(client_manager_tx.clone(), client_manager_rx);

    for stream in tcp_listener.incoming() {
        match stream {
            Ok(stream) => {
                stream.set_nonblocking(true).expect("Error setting stream to non-blocking");
                client_manager_tx.try_send(ClientManagerMessage::Connect { stream }).unwrap()
            }
            _ => {}
        }
    }
}

fn start_telnet_server() -> TcpListener {
    let local_ip_address = local_ip().unwrap();
    println!("{}", local_ip_address);
    let address = local_ip_address.to_string() + ":9000";
    let listener = TcpListener::bind(address.clone()).unwrap();
    listener.set_nonblocking(true).expect("Cannot set non-blocking");
    println!("Telnet Server Listening on: {}", address);
    listener
}

fn launch_client_manager(sender: Sender<ClientManagerMessage>, receiver: Receiver<ClientManagerMessage>) {
    let client_manager = ClientManager::new(receiver);
    let _ = thread::spawn(
        move || {
            loop {
                match client_manager.receive() {
                    Ok(client_manager_message) => {
                        match client_manager_message {
                            ClientManagerMessage::Connect { stream } => {
                                println!("TCP Connect event received");
                                // TODO Log connection
                                // TODO: Validate IP address before connecting to server
                                let client_manager_sender = sender.clone();
                                let client_id = Uuid::new_v4();
                                let client_connection = create_client_connection(client_id, stream, client_manager_sender);
                                println!("Client Connection created - Client ID: {} | Client IP Address: {}", client_id, client_connection.ip_addr);
                                if client_id == client_connection.client_id {
                                    let _ = client_manager.clients.insert(client_id, client_connection);
                                    println!("Inserted Client ID: {} to into Client Map", client_id)
                                }
                            }
                            ClientManagerMessage::ConnectionClosed { client_id } => {
                                // TODO Log message received from client
                                match client_manager.clients.get(client_id) {
                                    Some(client_connection) => {
                                        if client_id == client_connection.client_id {
                                            let _ = client_manager.clients.remove(client_id);
                                            println!("Client ID: {} removed from client map.", client_id);
                                        }
                                    }
                                    _ => { println!("No Client Mapping Data for Client ID: {}", client_id) }
                                };
                            }
                        }
                    }
                    _ => {}
                }
                sleep(Duration::from_nanos(10))
            }
        }
    );
}

fn create_client_connection(client_id: uuid::Uuid, stream: TcpStream, client_manager_tx: Sender<ClientManagerMessage>) -> ClientConnection {
    let ip_addr = stream.peer_addr().unwrap().ip();
    let client_connection = ClientConnection { client_id, ip_addr };
    let mut _stream = stream.try_clone().expect("clone failed...");
    let _ = thread::spawn(
        move || {
            let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::from_str("0.0.0.0") //destination ip address
                .expect("Invalid address")), 2727);
            let buffer_size = 256;
            let mut telnet = Telnet::connect_timeout(&address, buffer_size, Duration::from_secs(10))
                .expect("Couldn't connect to the server...");
            let client_id = client_id;
            println!("Client ID: {} connected to Telnet Server", client_id);
            loop {
                const MESSAGE_SIZE: usize = 1;
                let mut rx_bytes = [0u8; MESSAGE_SIZE];
                match _stream.read(&mut rx_bytes) {
                    Ok(bytes_read) => {
                        if bytes_read > 0 {
                            telnet.write(&rx_bytes).unwrap();
                        }
                    }
                    _ => {}
                }

                let event = telnet.read_nonblocking().expect("Telnet Read Error");
                match event {
                    TelnetEvent::Data(buffer) => {
                        let response = String::from_cp437(buffer.into_vec(), &CP437_CONTROL);
                        _stream.write_all(response.as_bytes()).expect("TCP Stream Write All Error");
                        _stream.flush().expect("TCP Stream Flush Error");
                    }
                    TelnetEvent::Negotiation(action, option) => {
                        // Action Do, Option: SNDLOC
                        // Action Will, Option: Echo -- default is to not echo
                        // Action Will, Option: SuppressGoAhead -- default is not to supress go ahead
                        // Action Do, Option: TransmitBinary --- default is to not transmit binary
                        // Action Do, Option: TTYPE

                        match option {
                            TelnetOption::SNDLOC => {
                                match action {
                                    Action::Do => {
                                        telnet.negotiate(&Action::Will, option).unwrap();

                                        let location = String::from(ip_addr.to_string());
                                        telnet.subnegotiate(TelnetOption::SNDLOC, &location.as_bytes().to_vec()).unwrap();
                                    }
                                    Action::Dont => {
                                        telnet.negotiate(&Action::Wont, option).unwrap();
                                    }
                                    Action::Will => {
                                        telnet.negotiate(&Action::Dont, option).unwrap();
                                    }
                                    Action::Wont => {
                                        telnet.negotiate(&Action::Dont, option).unwrap();
                                    }
                                }
                            }
                            TelnetOption::Echo => {
                                match action {
                                    Action::Do => {
                                        telnet.negotiate(&Action::Will, option).unwrap();
                                    }
                                    Action::Dont => {
                                        telnet.negotiate(&Action::Wont, option).unwrap();
                                    }
                                    Action::Will => {
                                        telnet.negotiate(&Action::Do, option).unwrap();
                                    }
                                    Action::Wont => {
                                        telnet.negotiate(&Action::Dont, option).unwrap();
                                    }
                                }
                            }
                            TelnetOption::SuppressGoAhead => {
                                match action {
                                    Action::Do => {
                                        telnet.negotiate(&Action::Will, option).unwrap();
                                    }
                                    Action::Dont => {
                                        telnet.negotiate(&Action::Will, option).unwrap();
                                    }
                                    Action::Will => {
                                        telnet.negotiate(&Action::Do, option).unwrap();
                                    }
                                    Action::Wont => {
                                        telnet.negotiate(&Action::Do, option).unwrap();
                                    }
                                }
                            }
                            TelnetOption::TransmitBinary => {
                                match action {
                                    Action::Do => {
                                        telnet.negotiate(&Action::Will, option).unwrap();
                                    }
                                    Action::Dont => {
                                        telnet.negotiate(&Action::Will, option).unwrap();
                                    }
                                    Action::Will => {
                                        telnet.negotiate(&Action::Do, option).unwrap();
                                    }
                                    Action::Wont => {
                                        telnet.negotiate(&Action::Do, option).unwrap();
                                    }
                                }
                            }
                            TelnetOption::TTYPE => {
                                match action {
                                    Action::Do => {
                                        telnet.negotiate(&Action::Will, option).unwrap();
                                        // TODO: Send actual terminal type
                                        let terminal_type = String::from("ansi-bbs");
                                        telnet.subnegotiate(TelnetOption::TTYPE, &terminal_type.as_bytes().to_vec()).unwrap();
                                    }
                                    _ => ()
                                }
                            }
                            _ => ()
                        }
                    }
                    TelnetEvent::Error(error) => {
                        match error {
                            TelnetError::InternalQueueErr => {
                                println!("Internal Queue Error Occurred for Client ID: {}. Transmitting close message.", client_id);
                                break;
                            }
                            _ => {}
                        }
                    }
                    TelnetEvent::UnknownIAC(_) => {}
                    TelnetEvent::Subnegotiation(_, _) => {}
                    TelnetEvent::TimedOut => { println!("Timed out") }
                    TelnetEvent::NoData => {}
                }
                sleep(Duration::from_nanos(10))
            }
            client_manager_tx.try_send(ClientManagerMessage::ConnectionClosed { client_id }).unwrap();
            println!("Client ID: {} - Telnet Connection Closed", client_id)
        }
    );
    client_connection
}
