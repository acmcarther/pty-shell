#![deny(missing_debug_implementations,
        trivial_casts, trivial_numeric_casts,
        unstable_features,
        unused_import_braces, unused_qualifications)]

extern crate libc;
extern crate nix;
extern crate pty;
extern crate termios;
extern crate mio;

use std::{fmt, io, result, thread};
use std::io::{Read, Write};

pub use self::error::*;
pub use self::terminal::*;
use self::raw_handler::*;
use self::winsize::Winsize;

pub mod error;
pub mod terminal;
pub mod winsize;

mod raw_handler;
mod command;

pub type Result<T> = result::Result<T, Error>;

pub trait PtyHandler {
    fn input(&mut self, _data: &[u8]) {}
    fn output(&mut self, _data: &[u8]) {}
    fn resize(&mut self, _winsize: &Winsize) {}
    fn shutdown(&mut self) {}
}

pub trait PtyShell {
    fn exec<S: AsRef<str>>(&self, shell: S) -> Result<()>;
    fn proxy<H: PtyHandler>(&self, handler: H) -> Result<()>;
}

impl PtyShell for pty::Child {
    fn exec<S: AsRef<str>>(&self, shell: S) -> Result<()> {
        if self.pid() == 0 {
            command::exec(shell);
        }

        Ok(())
    }

    fn proxy<H: PtyHandler + 'static>(&self, handler: H) -> Result<()> {
        if self.pid() != 0 {
            try!(setup_terminal(self.pty().unwrap()));
            try!(do_proxy(self.pty().unwrap(), handler));
        }

        Ok(())
    }
}

fn do_proxy<H: PtyHandler + 'static>(pty: pty::ChildPTY, handler: H) -> Result<()> {
    let mut event_loop = try!(mio::EventLoop::new());

    let mut writer = pty.clone();
    let (input_reader, mut input_writer) = try!(mio::unix::pipe());

    /*
    thread::spawn(move || {
        handle_input(&mut writer, &mut input_writer).unwrap_or_else(|e| {
            println!("{:?}", e);
        });
    });
    */

    let mut reader = pty.clone();
    let (output_reader, mut output_writer) = try!(mio::unix::pipe());
    let message_sender = event_loop.channel();

    thread::spawn(move || {
        handle_output(&mut reader, &mut output_writer).unwrap_or_else(|e| {
            println!("{:?}", e);
        });

        message_sender.send(Message::Shutdown).unwrap();
    });

    try!(event_loop.register(&input_reader, INPUT, mio::EventSet::readable(), mio::PollOpt::level()));
    try!(event_loop.register(&output_reader, OUTPUT, mio::EventSet::readable(), mio::PollOpt::level()));
    RawHandler::register_sigwinch_handler();

    let mut raw_handler = RawHandler::new(input_reader, output_reader, pty.clone(), Box::new(handler));

    thread::spawn(move || {
        event_loop.run(&mut raw_handler).unwrap_or_else(|e| {
            println!("{:?}", e);
        });
    });

    Ok(())
}

/*
fn handle_input(writer: &mut pty::ChildPTY, handler_writer: &mut mio::unix::PipeWriter) -> Result<()> {
    let mut input = io::stdin();
    let mut buf   = [0; 128];

    loop {
        let nread = try!(input.read(&mut buf));

        try!(writer.write(&buf[..nread]));
        try!(handler_writer.write(&buf[..nread]));
    }
}
*/

fn handle_output(reader: &mut pty::ChildPTY, handler_writer: &mut mio::unix::PipeWriter) -> Result<()> {
    let mut buf    = [0; 1024 * 10];

    loop {
        let nread = try!(reader.read(&mut buf));

        if nread <= 0 {
            break;
        } else {
            try!(handler_writer.write(&buf[..nread]));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate pty;

    use PtyShell;
    use PtyHandler;
    use terminal::restore_termios;

    struct TestHandler;
    impl PtyHandler for TestHandler {
        fn output(&mut self, data: &[u8]) {
            assert!(data.len() != 0);
        }
    }

    #[test]
    fn it_can_spawn() {
        let child = pty::fork().unwrap();
        restore_termios();

        child.exec("pwd").unwrap();

        assert!(child.wait().is_ok());
    }

    #[test]
    fn it_can_hook_stdout_with_handler() {
      use std::io::Write;
        let child = pty::fork().unwrap();
        restore_termios();

        child.proxy(TestHandler).unwrap();
        child.exec("bash").unwrap();

        child.pty().unwrap().write(b"exit\n");

        assert!(child.wait().is_ok());
    }
}
