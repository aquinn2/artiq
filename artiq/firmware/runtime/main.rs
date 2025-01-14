#![feature(lang_items, alloc, try_from, nonzero, asm,
           panic_implementation, panic_info_message,
           const_slice_len)]
#![no_std]

extern crate eh;
#[macro_use]
extern crate alloc;
extern crate failure;
#[macro_use]
extern crate failure_derive;
extern crate cslice;
#[macro_use]
extern crate log;
extern crate byteorder;
extern crate fringe;
extern crate managed;
extern crate smoltcp;

extern crate alloc_list;
extern crate unwind_backtrace;
extern crate io;
#[macro_use]
extern crate board_misoc;
extern crate board_artiq;
extern crate logger_artiq;
extern crate proto_artiq;

use core::cell::RefCell;
use core::convert::TryFrom;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr};

use board_misoc::{csr, irq, ident, clock, boot, config};
#[cfg(has_ethmac)]
use board_misoc::ethmac;
#[cfg(has_drtio)]
use board_artiq::drtioaux;
use board_artiq::drtio_routing;
use board_artiq::{mailbox, rpc_queue};
use proto_artiq::{mgmt_proto, moninj_proto, rpc_proto, session_proto, kernel_proto};
#[cfg(has_rtio_analyzer)]
use proto_artiq::analyzer_proto;

mod rtio_clocking;
mod rtio_mgt;

mod urc;
mod sched;
mod cache;
mod rtio_dma;

mod mgmt;
mod profiler;
mod kernel;
mod kern_hwreq;
mod watchdog;
mod session;
#[cfg(any(has_rtio_moninj, has_drtio))]
mod moninj;
#[cfg(has_rtio_analyzer)]
mod analyzer;

#[cfg(has_grabber)]
fn grabber_thread(io: sched::Io) {
    loop {
        board_artiq::grabber::tick();
        io.sleep(200).unwrap();
    }
}

fn setup_log_levels() {
    match config::read_str("log_level", |r| r.map(|s| s.parse())) {
        Ok(Ok(log_level_filter)) => {
            info!("log level set to {} by `log_level` config key",
                  log_level_filter);
            log::set_max_level(log_level_filter);
        }
        _ => info!("log level set to INFO by default")
    }
    match config::read_str("uart_log_level", |r| r.map(|s| s.parse())) {
        Ok(Ok(uart_log_level_filter)) => {
            info!("UART log level set to {} by `uart_log_level` config key",
                  uart_log_level_filter);
            logger_artiq::BufferLogger::with(|logger|
                logger.set_uart_log_level(uart_log_level_filter));
        }
        _ => info!("UART log level set to INFO by default")
    }
}

fn sayma_hw_init() {
    #[cfg(has_slave_fpga_cfg)]
    board_artiq::slave_fpga::load().expect("cannot load RTM FPGA gateware");
    #[cfg(has_serwb_phy_amc)]
    board_artiq::serwb::wait_init();

    #[cfg(has_hmc830_7043)]
    /* must be the first SPI init because of HMC830 SPI mode selection */
    board_artiq::hmc830_7043::init().expect("cannot initialize HMC830/7043");
    #[cfg(has_ad9154)]
    {
        board_artiq::ad9154::jesd_reset(false);
        board_artiq::ad9154::init();
        if let Err(e) = board_artiq::jesd204sync::sysref_auto_rtio_align() {
            error!("failed to align SYSREF at FPGA: {}", e);
        }
        if let Err(e) = board_artiq::jesd204sync::sysref_auto_dac_align() {
            error!("failed to align SYSREF at DAC: {}", e);
        }
    }
    #[cfg(has_allaki_atts)]
    board_artiq::hmc542::program_all(8/*=4dB*/);
}

fn startup() {
    irq::set_mask(0);
    irq::set_ie(true);
    clock::init();
    info!("ARTIQ runtime starting...");
    info!("software ident {}", csr::CONFIG_IDENTIFIER_STR);
    info!("gateware ident {}", ident::read(&mut [0; 64]));

    setup_log_levels();
    #[cfg(has_i2c)]
    board_artiq::i2c::init();
    sayma_hw_init();
    rtio_clocking::init();

    let hardware_addr;
    match config::read_str("mac", |r| r.map(|s| s.parse())) {
        Ok(Ok(addr)) => {
            hardware_addr = addr;
            info!("using MAC address {}", hardware_addr);
        }
        _ => {
            #[cfg(soc_platform = "kasli")]
            {
                let eeprom = board_artiq::i2c_eeprom::EEPROM::kasli_eeprom();
                hardware_addr =
                    eeprom.read_eui48()
                    .map(|addr_buf| {
                        let hardware_addr = EthernetAddress(addr_buf);
                        info!("using MAC address {} from EEPROM", hardware_addr);
                        hardware_addr
                    })
                    .unwrap_or_else(|e| {
                        error!("failed to read MAC address from EEPROM: {}", e);
                        let hardware_addr = EthernetAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x21]);
                        warn!("using default MAC address {}; consider changing it", hardware_addr);
                        hardware_addr
                    });
            }
            #[cfg(soc_platform = "sayma_amc")]
            {
                hardware_addr = EthernetAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x11]);
                warn!("using default MAC address {}; consider changing it", hardware_addr);
            }
            #[cfg(soc_platform = "metlino")]
            {
                hardware_addr = EthernetAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x19]);
                warn!("using default MAC address {}; consider changing it", hardware_addr);
            }
            #[cfg(soc_platform = "kc705")]
            {
                hardware_addr = EthernetAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
                warn!("using default MAC address {}; consider changing it", hardware_addr);
            }
        }
    }

    let protocol_addr;
    match config::read_str("ip", |r| r.map(|s| s.parse())) {
        Ok(Ok(addr)) => {
            protocol_addr = addr;
            info!("using IP address {}", protocol_addr);
        }
        _ => {
            #[cfg(soc_platform = "kasli")]
            {
                protocol_addr = IpAddress::v4(192, 168, 1, 70);
            }
            #[cfg(soc_platform = "sayma_amc")]
            {
                protocol_addr = IpAddress::v4(192, 168, 1, 60);
            }
            #[cfg(soc_platform = "metlino")]
            {
                protocol_addr = IpAddress::v4(192, 168, 1, 65);
            }
            #[cfg(soc_platform = "kc705")]
            {
                protocol_addr = IpAddress::v4(192, 168, 1, 50);
            }
            info!("using default IP address {}", protocol_addr);
        }
    }

    let mut net_device = unsafe { ethmac::EthernetDevice::new() };
    net_device.reset_phy_if_any();

    let net_device = {
        use smoltcp::time::Instant;
        use smoltcp::wire::PrettyPrinter;
        use smoltcp::wire::EthernetFrame;

        fn net_trace_writer(timestamp: Instant, printer: PrettyPrinter<EthernetFrame<&[u8]>>) {
            print!("\x1b[37m[{:6}.{:03}s]\n{}\x1b[0m\n",
                   timestamp.secs(), timestamp.millis(), printer)
        }

        fn net_trace_silent(_timestamp: Instant, _printer: PrettyPrinter<EthernetFrame<&[u8]>>) {}

        let net_trace_fn: fn(Instant, PrettyPrinter<EthernetFrame<&[u8]>>);
        match config::read_str("net_trace", |r| r.map(|s| s == "1")) {
            Ok(true) => net_trace_fn = net_trace_writer,
            _ => net_trace_fn = net_trace_silent
        }
        smoltcp::phy::EthernetTracer::new(net_device, net_trace_fn)
    };

    let neighbor_cache =
        smoltcp::iface::NeighborCache::new(alloc::btree_map::BTreeMap::new());
    let mut interface  =
        smoltcp::iface::EthernetInterfaceBuilder::new(net_device)
                       .neighbor_cache(neighbor_cache)
                       .ethernet_addr(hardware_addr)
                       .ip_addrs([IpCidr::new(protocol_addr, 0)])
                       .finalize();

    #[cfg(has_drtio)]
    let drtio_routing_table = urc::Urc::new(RefCell::new(
        drtio_routing::config_routing_table(csr::DRTIO.len())));
    #[cfg(not(has_drtio))]
    let drtio_routing_table = urc::Urc::new(RefCell::new(
        drtio_routing::RoutingTable::default_empty()));
    let up_destinations = urc::Urc::new(RefCell::new(
        [false; drtio_routing::DEST_COUNT]));
    #[cfg(has_drtio_routing)]
    drtio_routing::interconnect_disable_all();
    let aux_mutex = sched::Mutex::new();

    let mut scheduler = sched::Scheduler::new();
    let io = scheduler.io();

    rtio_mgt::startup(&io, &aux_mutex, &drtio_routing_table, &up_destinations);

    io.spawn(4096, mgmt::thread);
    {
        let aux_mutex = aux_mutex.clone();
        let drtio_routing_table = drtio_routing_table.clone();
        let up_destinations = up_destinations.clone();
        io.spawn(16384, move |io| { session::thread(io, &aux_mutex, &drtio_routing_table, &up_destinations) });
    }
    #[cfg(any(has_rtio_moninj, has_drtio))]
    {
        let aux_mutex = aux_mutex.clone();
        let drtio_routing_table = drtio_routing_table.clone();
        io.spawn(4096, move |io| { moninj::thread(io, &aux_mutex, &drtio_routing_table) });
    }
    #[cfg(has_rtio_analyzer)]
    io.spawn(4096, analyzer::thread);

    #[cfg(has_grabber)]
    io.spawn(4096, grabber_thread);

    let mut net_stats = ethmac::EthernetStatistics::new();
    loop {
        scheduler.run();

        {
            let sockets = &mut *scheduler.sockets().borrow_mut();
            loop {
                let timestamp = smoltcp::time::Instant::from_millis(clock::get_ms() as i64);
                match interface.poll(sockets, timestamp) {
                    Ok(true) => (),
                    Ok(false) => break,
                    Err(smoltcp::Error::Unrecognized) => (),
                    Err(err) => debug!("network error: {}", err)
                }
            }
        }

        if let Some(_net_stats_diff) = net_stats.update() {
            debug!("ethernet mac:{}", ethmac::EthernetStatistics::new());
        }
    }
}

#[global_allocator]
static mut ALLOC: alloc_list::ListAlloc = alloc_list::EMPTY;
static mut LOG_BUFFER: [u8; 1<<17] = [0; 1<<17];

#[no_mangle]
pub extern fn main() -> i32 {
    unsafe {
        extern {
            static mut _fheap: u8;
            static mut _eheap: u8;
        }
        ALLOC.add_range(&mut _fheap, &mut _eheap);

        logger_artiq::BufferLogger::new(&mut LOG_BUFFER[..]).register(startup);

        0
    }
}

#[no_mangle]
pub extern fn exception(vect: u32, _regs: *const u32, pc: u32, ea: u32) {
    let vect = irq::Exception::try_from(vect).expect("unknown exception");
    match vect {
        irq::Exception::Interrupt =>
            while irq::pending_mask() != 0 {
                match () {
                    #[cfg(has_timer1)]
                    () if irq::is_pending(csr::TIMER1_INTERRUPT) =>
                        profiler::sample(pc as usize),
                    _ => panic!("spurious irq {}", irq::pending_mask().trailing_zeros())
                }
            },
        _ => {
            fn hexdump(addr: u32) {
                let addr = (addr - addr % 4) as *const u32;
                let mut ptr  = addr;
                println!("@ {:08p}", ptr);
                for _ in 0..4 {
                    print!("+{:04x}: ", ptr as usize - addr as usize);
                    print!("{:08x} ",   unsafe { *ptr }); ptr = ptr.wrapping_offset(1);
                    print!("{:08x} ",   unsafe { *ptr }); ptr = ptr.wrapping_offset(1);
                    print!("{:08x} ",   unsafe { *ptr }); ptr = ptr.wrapping_offset(1);
                    print!("{:08x}\n",  unsafe { *ptr }); ptr = ptr.wrapping_offset(1);
                }
            }

            hexdump(pc);
            hexdump(ea);
            panic!("exception {:?} at PC 0x{:x}, EA 0x{:x}", vect, pc, ea)
        }
    }
}

#[no_mangle]
pub extern fn abort() {
    println!("aborted");
    loop {}
}

#[no_mangle] // https://github.com/rust-lang/rust/issues/{38281,51647}
#[lang = "oom"] // https://github.com/rust-lang/rust/issues/51540
pub fn oom(layout: core::alloc::Layout) -> ! {
    panic!("heap view: {}\ncannot allocate layout: {:?}", unsafe { &ALLOC }, layout)
}

#[no_mangle] // https://github.com/rust-lang/rust/issues/{38281,51647}
#[panic_implementation]
pub fn panic_impl(info: &core::panic::PanicInfo) -> ! {
    irq::set_ie(false);

    if let Some(location) = info.location() {
        print!("panic at {}:{}:{}", location.file(), location.line(), location.column());
    } else {
        print!("panic at unknown location");
    }
    if let Some(message) = info.message() {
        println!("{}", message);
    } else {
        println!("");
    }

    println!("backtrace for software version {}:", csr::CONFIG_IDENTIFIER_STR);
    let _ = unwind_backtrace::backtrace(|ip| {
        // Backtrace gives us the return address, i.e. the address after the delay slot,
        // but we're interested in the call instruction.
        println!("{:#08x}", ip - 2 * 4);
    });

    if config::read_str("panic_reset", |r| r == Ok("1")) {
        println!("restarting...");
        unsafe { boot::reset() }
    } else {
        println!("halting.");
        println!("use `artiq_coreconfig write -s panic_reset 1` to restart instead");
        loop {}
    }
}
