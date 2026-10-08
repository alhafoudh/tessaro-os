//! What a device's answers say, in words, for both clients: `tessaro-ctl`
//! prints these lines and facts, the GUI shows them on its pages. Pure
//! functions of what the device sent; nothing here talks to it.

pub mod audio;
pub mod browser;
pub mod bulk;
pub mod camera;
pub mod cec;
pub mod device;
pub mod net;
pub mod playlist;
pub mod printer;
pub mod screen;
pub mod time;
