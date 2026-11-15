use std::sync::OnceLock;

use zbus::blocking::{Connection, Proxy, connection};
use zbus::fdo::RequestNameFlags;

const NAME: &str = "phonelink.linux.App";
const PATH: &str = "/phonelink/linux/App";

type Show = Box<dyn Fn() + Send + Sync>;

static SHOW: OnceLock<Show> = OnceLock::new();

struct App;

#[zbus::interface(name = "phonelink.linux.App")]
impl App {
    fn show(&self) {
        if let Some(show) = SHOW.get() {
            show();
        }
    }
}

pub enum Claim {
    First(Option<Connection>),
    Running,
}

pub fn claim() -> Claim {
    let first = connection::Builder::session().and_then(|builder| builder.serve_at(PATH, App)?.build()).and_then(
        |connection| {
            connection.request_name_with_flags(NAME, RequestNameFlags::DoNotQueue.into())?;
            Ok(connection)
        },
    );
    match first {
        Ok(connection) => Claim::First(Some(connection)),
        Err(zbus::Error::NameTaken) => {
            let shown =
                Connection::session().and_then(|bus| Proxy::new(&bus, NAME, PATH, NAME)?.call::<_, _, ()>("Show", &()));
            match shown {
                Ok(()) => Claim::Running,
                Err(e) => {
                    eprintln!("the running Phone Link did not answer: {e}");
                    Claim::First(None)
                }
            }
        }
        Err(e) => {
            eprintln!("Phone Link does not guard against a second copy: {e}");
            Claim::First(None)
        }
    }
}

pub fn on_show(show: impl Fn() + Send + Sync + 'static) {
    let _ = SHOW.set(Box::new(show));
}
