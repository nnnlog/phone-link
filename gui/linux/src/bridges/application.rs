//! The C++ application object: created before the QML engine, run, and destroyed after it.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("phonelink-linux/src/application.h");

        #[namespace = "phonelink"]
        #[cxx_name = "createApplication"]
        fn create_application(arguments: &Vec<String>);

        #[namespace = "phonelink"]
        #[cxx_name = "execApplication"]
        fn exec_application() -> i32;

        #[namespace = "phonelink"]
        #[cxx_name = "destroyApplication"]
        fn destroy_application();
    }
}

pub use qobject::{create_application, destroy_application, exec_application};
