use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use mpris::PlayerFinder;
use tanuki::{
    TanukiConnection,
    capabilities::{Authority, media::Media},
    common::capabilities::media::{MediaInfo, MediaState, MediaStatus},
};

// TODO: probably rewrite using direct dbus instead of this mpris crate

#[derive(Parser)]
struct Args {
    /// Tanuki entity ID
    entity_id: String,

    /// Tanuki MQTT broker address
    mqtt_addr: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let finder = PlayerFinder::new().context("failed to connect to D-Bus")?;

    let tanuki = TanukiConnection::connect("tanuki-mpris", &args.mqtt_addr)
        .await
        .context("failed to connect to tanuki mqtt broker")?;

    'find_active: loop {
        let player = match finder.find_active() {
            Ok(player) => player,
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue 'find_active;
            }
        };

        eprintln!("Found active player: {}", player.bus_name());

        handle_player(tanuki.clone(), &args, &player).await?;
    }
}

async fn handle_player(
    tanuki: Arc<TanukiConnection>,
    args: &Args,
    player: &mpris::Player,
) -> anyhow::Result<()> {
    let entity = tanuki.entity(&args.entity_id).await?;
    let tanuki_media = entity.capability::<Media<Authority>>().await?;

    let mut state = MediaState::default();
    state.status = match player.get_playback_status()? {
        mpris::PlaybackStatus::Playing => MediaStatus::Playing,
        mpris::PlaybackStatus::Paused => MediaStatus::Paused,
        mpris::PlaybackStatus::Stopped => MediaStatus::Stopped,
    };

    if let Ok(metadata) = player.get_metadata() {
        eprintln!("Metadata: {:#?}", metadata);

        state.info = metadata_to_info(&metadata);

        tanuki_media.publish(state.clone()).await?;
    }

    // TODO
    // let mut progress = player.track_progress(200)?;
    // loop {
    //     let tick = progress.tick();
    //     eprintln!("{:#?}", tick.progress.position());
    // }

    for ev in player.events()? {
        eprintln!("Player event: {:#?}", ev);

        let ev = ev?;

        match ev {
            mpris::Event::Paused => state.status = MediaStatus::Paused,
            mpris::Event::Playing => state.status = MediaStatus::Playing,
            mpris::Event::Stopped => state.status = MediaStatus::Stopped,

            mpris::Event::TrackChanged(metadata) => state.info = metadata_to_info(&metadata),

            // TODO
            mpris::Event::PlayerShutDown => continue,
            mpris::Event::LoopingChanged(_loop_status) => continue,
            mpris::Event::ShuffleToggled(_) => continue,
            mpris::Event::VolumeChanged(_) => continue,
            mpris::Event::PlaybackRateChanged(_) => continue,
            mpris::Event::Seeked { position_in_us: _ } => continue,
            mpris::Event::TrackAdded(_track_id) => continue,
            mpris::Event::TrackRemoved(_track_id) => continue,
            mpris::Event::TrackMetadataChanged { old_id: _, new_id: _ } => continue,
            mpris::Event::TrackListReplaced => continue,
        }

        // TODO: can we pass a reference instead?
        tanuki_media.publish(state.clone()).await?;
    }

    eprintln!("Player has shut down");

    Ok(())
}

fn metadata_to_info(metadata: &mpris::Metadata) -> MediaInfo {
    let mut info = MediaInfo::default();
    info.title = metadata.title().map(|s| s.to_string());
    info.artists = metadata
        .artists()
        .map(|artists| artists.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    info.album = metadata.album_name().map(|s| s.to_string());
    info.url = metadata.url().map(|s| s.to_string());
    info
}
