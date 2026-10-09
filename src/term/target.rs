//! An Objective-C target that runs a Rust closure, for buttons, menu items
//! and search fields. Controls hold targets weakly: keep it alive yourself.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};

pub struct Ivars {
    action: Box<dyn Fn(Option<&AnyObject>)>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct Target;

    impl Target {
        #[unsafe(method(fire:))]
        fn fire(&self, sender: Option<&AnyObject>) {
            (self.ivars().action)(sender);
        }
    }
);

impl Target {
    pub fn new(mtm: MainThreadMarker, action: impl Fn(Option<&AnyObject>) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { action: Box::new(action) });
        unsafe { msg_send![super(this), init] }
    }

    pub fn action() -> Sel {
        sel!(fire:)
    }
}
