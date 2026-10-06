//! Shared Tanuki wire codecs and validated, transport-independent primitives.
mod identity;
mod path;
mod requests;
mod value;
mod wire;
pub use identity::*;
pub use path::*;
pub use requests::*;
pub use value::*;
pub use wire::*;
mod view;
pub use view::{AppliedUpdate, ClientViewError, SelectedView};
mod decode;
