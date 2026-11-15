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
    }
}

pub use qobject::{create_application, exec_application};
