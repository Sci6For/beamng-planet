use super::*;

fn vehicle(game_id: u32) -> VehicleData {
    VehicleData {
        in_game_id: game_id, server_id: 0, owner: None,
        name: "pickup".into(), parts_config: "{}".into(), plate: None,
        color: [0.0; 8], palete_0: [0.0; 8], palete_1: [0.0; 8],
        position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0],
    }
}

fn update(game_id: u32, generation: u64, x: f32) -> IncomingEvent {
    IncomingEvent::ClientCommand(shared::ClientCommand::VehicleUpdate(VehicleUpdate {
        vehicle_id: game_id, generation, sent_at: generation as f64,
        transform: Transform {position: [x, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0],
            velocity: [x, 0.0, 0.0], angular_velocity: [0.0; 3]},
        electrics: Electrics {throttle_input: x, brake_input: 0.0, clutch: 0.0,
            parkingbrake: 0.0, steering_input: 0.0},
        gearbox: Gearbox {lock_coef: x, grb_bhv: None, grb_idx: 1, grb_mde: None, frzn: false},
    }))
}

fn server() -> Server {
    Server::from_config(config::Config {mods: Some(vec![]), ..Default::default()})
}

#[test]
fn shutdown_without_upnp_mapping_does_not_wait_for_gateway_discovery() {
    let server = server();
    let start = std::time::Instant::now();
    drop(server);
    assert!(start.elapsed() < Duration::from_secs(2), "unmapped shutdown waited for router discovery");
}

#[tokio::test]
async fn first_zero_is_accepted_but_older_and_duplicate_snapshots_do_not_mutate_state() {
    let mut server = server();
    server.spawn_vehicle(Some(42), vehicle(1)).await;
    let id = server.get_server_id_from_game_id(42, 1).unwrap();
    server.on_client_event(42, update(1, 0, 10.0)).await;
    assert_eq!(server.vehicles[&id].transform.as_ref().unwrap().position[0], 10.0);
    server.on_client_event(42, update(1, 4, 40.0)).await;
    for (generation, x) in [(3, 30.0), (4, 99.0)] {
        server.on_client_event(42, update(1, generation, x)).await;
        let current = &server.vehicles[&id];
        assert_eq!(current.data.position[0], 40.0);
        assert_eq!(current.transform.as_ref().unwrap().velocity[0], 40.0);
        assert_eq!(current.electrics.as_ref().unwrap().throttle_input, 40.0);
        assert_eq!(current.gearbox.as_ref().unwrap().lock_coef, 40.0);
    }
}

#[tokio::test]
async fn sequence_history_is_per_vehicle_and_does_not_survive_replacement() {
    let mut server = server();
    server.spawn_vehicle(Some(42), vehicle(1)).await;
    server.spawn_vehicle(Some(42), vehicle(2)).await;
    let first = server.get_server_id_from_game_id(42, 1).unwrap();
    let second = server.get_server_id_from_game_id(42, 2).unwrap();
    server.on_client_event(42, update(1, 100, 50.0)).await;
    server.on_client_event(42, update(2, 0, 20.0)).await;
    assert_eq!(server.vehicles[&second].data.position[0], 20.0);
    server.on_client_event(43, update(1, 200, 999.0)).await;
    assert_eq!(server.vehicles[&first].data.position[0], 50.0);
    server.remove_vehicle(first, Some(42)).await;
    server.spawn_vehicle(Some(42), vehicle(1)).await;
    let replacement = server.get_server_id_from_game_id(42, 1).unwrap();
    server.on_client_event(42, update(1, 0, 5.0)).await;
    assert_eq!(server.vehicles[&replacement].data.position[0], 5.0);
}
