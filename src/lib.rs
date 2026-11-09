//! GUI バイナリと結合テストで共有するアプリケーションモジュール。
//!
//! パッケージの主成果物は `groq-whisper-app` バイナリであり、公開モジュールは
//! 安定した外部ライブラリ API を約束するものではない。結合テストから GUI を
//! 起動せずに音声エンコードと HTTP 送信を検証できるよう、ライブラリ target として
//! も公開する。

pub mod app;
pub mod audio;
pub mod config;
pub mod encoder;
#[cfg(test)]
mod fixture;
pub mod hotkey;
pub mod hotkey_listener;
pub mod persistence;
pub mod transcriber;
pub mod ui;
pub mod usage;
