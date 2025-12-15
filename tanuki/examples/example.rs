#[tokio::main]
async fn main() {
    tanuki::log::init();

    tokio::spawn(async move {
        tanuki_bthome::bridge("192.168.0.106:1883", [
            ("ATC_164B6D", "atc_balcony", "ATC Balcony"),
            ("ATC_2DB3D7", "atc_door_ceiling", "ATC Door Ceiling"),
        ])
        .await
        .unwrap();
    });

    tokio::spawn(async move {
        tanuki_hass::bridge(
            "192.168.0.106:1883",
            std::env::var("HASS_HOST").unwrap().as_str(),
            std::env::var("HASS_TOKEN").unwrap().as_str(),
        )
        .await
        .unwrap();
    });

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}
