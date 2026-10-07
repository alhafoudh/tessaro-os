//! tessaro-vision's pieces, as a library so the tests can run a model on a
//! picture without a camera: BlazeFace's anchors and decoding, a frame as
//! the network's input, the network, and following faces between frames.

pub mod blazeface;
pub mod detector;
pub mod picture;
pub mod track;
