//! Otoa ASR Protocol v1 のサーバー。
//!
//! バイナリとしても、他のバイナリへ組み込んでも使える。組み込めるように
//! してあるのは、**利用者に 2 つのプロセスを起動させないため**である。
//! クライアントは自分自身をサーバーとして起動できる。

mod asr;
mod audio;
mod config;
mod dump;
mod server;
mod session;

pub use config::{Config, USAGE};
pub use server::run;
