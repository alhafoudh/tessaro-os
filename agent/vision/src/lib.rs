//! tessaro-vision's pieces, as a library so the tests can run a model on a
//! picture without a camera: BlazeFace's anchors and decoding, a frame as
//! the network's input, the network, following faces between frames, and
//! their age and gender.

pub mod blazeface;
pub mod demographics;
pub mod detector;
pub mod picture;
pub mod track;
