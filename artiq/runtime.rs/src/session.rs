use std::prelude::v1::*;
use std::mem;
use std::str;
use std::io::{self, Read};
use {config, rtio_crg, clock, mailbox, kernel};
use logger::BufferLogger;
use cache::Cache;
use sched::{Waiter, TcpListener, TcpStream, SocketAddr, IP_ANY};

use session_proto as host;
use kernel_proto as kern;

macro_rules! unexpected {
    ($($arg:tt)*) => {
        {
            error!($($arg)*);
            return Err(io::Error::new(io::ErrorKind::InvalidData, "protocol error"))
        }
    };
}

fn io_error(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg)
}

// Persistent state
#[derive(Debug)]
struct Congress {
    now: u64,
    cache: Cache
}

impl Congress {
    fn new() -> Congress {
        Congress {
            now: 0,
            cache: Cache::new()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KernelState {
    Absent,
    Loaded,
    Running,
    RpcWait
}

// Per-connection state
#[derive(Debug)]
struct Session {
    kernel_state: KernelState,
    watchdog_set: clock::WatchdogSet
}

impl Session {
    fn new() -> Session {
        Session {
            kernel_state: KernelState::Absent,
            watchdog_set: clock::WatchdogSet::new()
        }
    }

    fn running(&self) -> bool {
        match self.kernel_state {
            KernelState::Absent  | KernelState::Loaded  => false,
            KernelState::Running | KernelState::RpcWait => true
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        kernel::stop()
    }
}

fn check_magic(stream: &mut TcpStream) -> io::Result<()> {
    const MAGIC: &'static [u8] = b"ARTIQ coredev\n";

    let mut magic: [u8; 14] = [0; 14];
    try!(stream.read_exact(&mut magic));
    if magic != MAGIC {
        Err(io::Error::new(io::ErrorKind::InvalidData, "unrecognized magic"))
    } else {
        Ok(())
    }
}

fn host_read(stream: &mut TcpStream) -> io::Result<host::Request> {
    let request = try!(host::Request::read_from(stream));
    match &request {
        &host::Request::LoadLibrary(_) => trace!("comm<-host LoadLibrary(...)"),
        _ => trace!("comm<-host {:?}", request)
    }
    Ok(request)
}

fn host_write(stream: &mut TcpStream, reply: host::Reply) -> io::Result<()> {
    trace!("comm->host {:?}", reply);
    reply.write_to(stream)
}

fn kern_send<'a>(waiter: Waiter, request: kern::Message<'a>) -> io::Result<()> {
    match &request {
        &kern::LoadRequest(_) => trace!("comm->kern LoadRequest(...)"),
        _ => trace!("comm->kern {:?}", request)
    }
    request.send_and_wait(waiter)
}

fn kern_recv<R, F>(waiter: Waiter, f: F) -> io::Result<R>
        where F: FnOnce(kern::Message) -> io::Result<R> {
    kern::Message::wait_and_receive(waiter, |reply| {
        trace!("comm<-kern {:?}", reply);
        f(reply)
    })
}

fn kern_acknowledge() -> io::Result<()> {
    kern::Message::acknowledge();
    Ok(())
}

fn comm_handle(logger: &BufferLogger,
               waiter: Waiter,
               stream: &mut TcpStream,
               session: &mut Session) -> io::Result<()> {
    match try!(host_read(stream)) {
        host::Request::Ident =>
            host_write(stream, host::Reply::Ident(::board::ident(&mut [0; 64]))),

        // artiq_corelog
        host::Request::Log => {
            // Logging the packet with the log is inadvisable
            trace!("comm->host Log(...)");
            logger.extract(move |log| {
                host::Reply::Log(log).write_to(stream)
            })
        }

        host::Request::LogClear => {
            logger.clear();
            host_write(stream, host::Reply::Log(""))
        }

        // artiq_coreconfig
        host::Request::FlashRead { ref key } => {
            let value = config::read_to_end(key);
            host_write(stream, host::Reply::FlashRead(&value))
        }

        host::Request::FlashWrite { ref key, ref value } => {
            match config::write(key, value) {
                Ok(_)  => host_write(stream, host::Reply::FlashOk),
                Err(_) => host_write(stream, host::Reply::FlashError)
            }
        }

        host::Request::FlashRemove { ref key } => {
            config::remove(key);
            host_write(stream, host::Reply::FlashOk)
        }

        host::Request::FlashErase => {
            config::erase();
            host_write(stream, host::Reply::FlashOk)
        }

        // artiq_run/artiq_master
        host::Request::SwitchClock(clk) => {
            if session.running() {
                error!("attempted to switch RTIO clock while a kernel was running");
                return host_write(stream, host::Reply::ClockSwitchFailed)
            }

            if rtio_crg::switch_clock(clk) {
                host_write(stream, host::Reply::ClockSwitchCompleted)
            } else {
                host_write(stream, host::Reply::ClockSwitchFailed)
            }
        }

        host::Request::LoadLibrary(library) => {
            if session.running() {
                error!("attempted to load a new kernel while a kernel was running");
                return host_write(stream, host::Reply::LoadFailed)
            }

            unsafe { kernel::start() }

            try!(kern_send(waiter, kern::LoadRequest(&library)));
            kern_recv(waiter, |reply| {
                match reply {
                    kern::LoadReply { error: None } => {
                        session.kernel_state = KernelState::Loaded;
                        host_write(stream, host::Reply::LoadCompleted)
                    }
                    kern::LoadReply { error: Some(cause) } => {
                        error!("cannot load kernel: {}", cause);
                        host_write(stream, host::Reply::LoadFailed)
                    }
                    other => unexpected!("unexpected reply from kernel CPU: {:?}", other)
                }
            })
        }

        host::Request::RunKernel => {
            if session.kernel_state != KernelState::Loaded {
                error!("attempted to run a kernel while not in Loaded state");
                return host_write(stream, host::Reply::KernelStartupFailed)
            }

            session.kernel_state = KernelState::Running;
            // TODO: make this a separate request
            kern_acknowledge()
        }

        request => unexpected!("unexpected request {:?} from host machine", request)
    }
}

fn kern_handle(waiter: Waiter,
               congress: &mut Congress,
               session: &mut Session) -> io::Result<()> {
    kern::Message::wait_and_receive(waiter, |request| {
        match (&request, session.kernel_state) {
            (&kern::LoadReply { .. }, KernelState::Loaded) |
            (&kern::RpcRecvRequest { .. }, KernelState::RpcWait) => {
                // We're standing by; ignore the message.
                return Ok(())
            }
            (_, KernelState::Running) => (),
            _ => {
                unexpected!("unexpected request {:?} from kernel CPU in {:?} state",
                            request, session.kernel_state)
            }
        }

        trace!("comm<-kern {:?}", request);
        match request {
            kern::Log(log) => {
                info!(target: "kernel", "{}", log);
                kern_acknowledge()
            }

            kern::NowInitRequest =>
                kern_send(waiter, kern::NowInitReply(congress.now)),

            kern::NowSave(now) => {
                congress.now = now;
                kern_acknowledge()
            }

            kern::WatchdogSetRequest { ms } => {
                let id = try!(session.watchdog_set.set_ms(ms)
                                .map_err(|()| io_error("out of watchdogs")));
                kern_send(waiter, kern::WatchdogSetReply { id: id })
            }

            kern::WatchdogClear { id } => {
                session.watchdog_set.clear(id);
                kern_acknowledge()
            }

            kern::CacheGetRequest { key } => {
                let value = congress.cache.get(key);
                kern_send(waiter, kern::CacheGetReply {
                    value: unsafe { mem::transmute::<*const [u32], &'static [u32]>(value) }
                })
            }

            kern::CachePutRequest { key, value } => {
                let succeeded = congress.cache.put(key, value).is_ok();
                kern_send(waiter, kern::CachePutReply { succeeded: succeeded })
            }

            request => unexpected!("unexpected request {:?} from kernel CPU", request)
        }
    })
}

fn handle(logger: &BufferLogger,
          waiter: Waiter,
          stream: &mut TcpStream,
          congress: &mut Congress) -> io::Result<()> {
    try!(check_magic(stream));

    let mut session = Session::new();
    loop {
        if stream.readable() {
            try!(comm_handle(logger, waiter, stream, &mut session))
        }

        if mailbox::receive() != 0 {
            try!(kern_handle(waiter, congress, &mut session))
        }

        if session.kernel_state == KernelState::Running {
            if session.watchdog_set.expired() {
                try!(host_write(stream, host::Reply::WatchdogExpired));
                return Err(io_error("watchdog expired"))
            }

            if !rtio_crg::check() {
                try!(host_write(stream, host::Reply::ClockFailure));
                return Err(io_error("RTIO clock failure"))
            }
        }

        waiter.relinquish()
    }
}

pub fn handler(waiter: Waiter,
               logger: &BufferLogger) {
    let mut congress = Congress::new();

    let addr = SocketAddr::new(IP_ANY, 1381);
    let listener = TcpListener::bind(waiter, addr).unwrap();
    info!("accepting network sessions in Rust");

    loop {
        let (mut stream, addr) = listener.accept().unwrap();
        info!("new connection from {:?}", addr);

        match handle(logger, waiter, &mut stream, &mut congress) {
            Ok(()) => (),
            Err(err) => {
                if err.kind() == io::ErrorKind::UnexpectedEof {
                    info!("connection closed");
                } else {
                    error!("session aborted: {:?}", err);
                }
            }
        }
    }
}
