#![forbid(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod msg_sign;
mod msg_verify;

pub use msg_sign::{MsgSign, MsgSignError};
pub use msg_verify::{MsgVerify, MsgVerifyError};
