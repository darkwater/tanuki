//! Shows every battery and the lamp, surviving server restarts.
use tanuki_client::{ClientError, Session, StateTopic, Status};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), ClientError> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ws://127.0.0.1:5167/v1/ws".into());
    // Stateless: any number of dashboards may share a name. `start` connects
    // in the background and keeps reconnecting.
    let session = Session::builder(&url, "example dashboard")
        .stateless()
        .start()?;
    let hue = StateTopic::<u16>::new("/lamp/hue")?;
    let brightness = StateTopic::<u8>::new("/lamp/brightness")?;
    let mut view = session.observe(("/battery/*", &hue, &brightness))?;
    let mut status = session.status();
    loop {
        tokio::select! {
            snapshot = view.recv() => {
                let Some(snapshot) = snapshot else { break };
                for (path, level) in snapshot.values::<u8>() {
                    if path.segments().next() == Some("battery") {
                        println!("{path}: {level:?}");
                    }
                }
                println!("lamp: hue {:?}, brightness {:?}", snapshot.get(&hue), snapshot.get(&brightness));
            }
            Some(change) = status.changed() => {
                if !matches!(change, Status::Connected) {
                    println!("({change:?}; showing the last known state)");
                }
            }
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result { eprintln!("signal error: {error}"); }
                break;
            }
        }
    }
    session.close().await;
    Ok(())
}
