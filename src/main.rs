#![allow(dead_code)]

mod miner;
mod pow;
mod protocol;
mod state;
mod strategy;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tungstenite::{connect, Message};
use uuid::Uuid;

use protocol::{ClientMsg, ServerMsg};
use crate::miner::MinerPool;
use crate::state::GameState;
use crate::strategy::{Strategy, NearestResourceStrategy};

// ─── Configuration ──────────────────────────────────────────────────────────

const SERVER_URL: &str = "ws://localhost:4004/ws";
const TEAM_NAME: &str = "Les mineurs";
const AGENT_NAME: &str = "mineur fou";
const NUM_MINERS: usize = 4;

fn main() {
    println!("[*] Connexion à {SERVER_URL}...");
    let (mut ws, _response) = connect(SERVER_URL).expect("impossible de se connecter au serveur");
    println!("[*] Connecté !");

    // ── Attendre le Hello ────────────────────────────────────────────────
    let agent_id: Uuid = match read_server_msg(&mut ws) {
        Some(ServerMsg::Hello { agent_id, tick_ms }) => {
            println!("[*] Hello reçu : agent_id={agent_id}, tick={tick_ms}ms");
            agent_id
        }
        other => panic!("premier message inattendu : {other:?}"),
    };

    // ── S'enregistrer ────────────────────────────────────────────────────
    send_client_msg(&mut ws, &ClientMsg::Register {
        team: TEAM_NAME.into(),
        name: AGENT_NAME.into(),
    });
    println!("[*] Enregistré en tant que {AGENT_NAME} (équipe {TEAM_NAME})");

    // ── Partie 1 — SharedState ───────────────────────────────────────────
    let shared_state: Arc<Mutex<GameState>> = state::new_shared_state(agent_id);

    // ── Partie 2 — MinerPool ─────────────────────────────────────────────
    let miner_pool = MinerPool::new(NUM_MINERS);

    // ── Partie 3 — Stratégie ─────────────────────────────────────────────
    // HybridStrategy : fonce vers la ressource la plus proche si elle est
    // connue, sinon explore la carte en balayage pour en trouver une.
    let strategy: Box<dyn Strategy> = Box::new(NearestResourceStrategy);

    // ── Partie 4 — Thread lecteur/écrivain WS ───────────────────────────
    //
    let (tx_in, rx_in)   = std::sync::mpsc::channel::<ServerMsg>();
    let (tx_out, rx_out) = std::sync::mpsc::channel::<ClientMsg>();

    let state_for_thread = Arc::clone(&shared_state);
    let tx_in_clone      = tx_in.clone();

    thread::spawn(move || {
        loop {
            // ── Écriture : vider d'abord tous les messages sortants ──────
            while let Ok(msg) = rx_out.try_recv() {
                send_client_msg(&mut ws, &msg);
            }

            // ── Lecture : parser tous les messages disponibles ───────────
            //
            match ws.read() {
                Ok(Message::Text(text)) => {
                    let mut de = serde_json::Deserializer::from_str(&text)
                        .into_iter::<ServerMsg>();
                    while let Some(Ok(msg)) = de.next() {
                        state_for_thread.lock().unwrap().update(&msg);
                        let _ = tx_in_clone.send(msg);
                    }
                }
                Ok(_) => {}
                Err(e) => eprintln!("[!] Erreur WS lecture : {e}"),
            }
        }
    });

    // ── Partie 5 — Boucle principale ────────────────────────────────────
    println!("[*] Démarrage de la boucle principale...");

    // On garde en mémoire le dernier tick pour lequel on a soumis un
    // challenge, afin d'éviter les doublons.
    let mut last_challenge_tick: u64 = 0;

    loop {
        // Traiter tous les messages reçus depuis le thread WS
        while let Ok(msg) = rx_in.try_recv() {
            match msg {
                ServerMsg::PowChallenge { tick, seed, resource_id, target_bits, .. } => {
                    // N'envoyer le challenge au pool que s'il est nouveau
                    if tick > last_challenge_tick {
                        println!("[*] PowChallenge : resource={resource_id}, tick={tick}, bits={target_bits}");
                        last_challenge_tick = tick;
                        miner_pool.submit(miner::MineRequest {
                            seed,
                            tick,
                            resource_id,
                            agent_id,
                            target_bits,
                        });
                        // Signaler au serveur qu'on commence à miner
                        let _ = tx_out.send(ClientMsg::Mining { resource_id, on: true });
                    }
                }
                ServerMsg::PowResult { resource_id, winner } => {
                    println!("[*] PowResult : ressource {resource_id} gagnée par {winner}");
                }
                ServerMsg::Win { team } => {
                    println!("[*] Victoire de l'équipe : {team} !");
                    return;
                }
                ServerMsg::Error { message } => {
                    eprintln!("[!] Erreur serveur : {message}");
                }
                _ => {}
            }
        }

        // Soumettre les nonces trouvés par le MinerPool
        while let Some(result) = miner_pool.try_recv() {
            println!("[*] Nonce trouvé : resource={}, nonce={}", result.resource_id, result.nonce);
            let _ = tx_out.send(ClientMsg::PowSubmit {
                tick: result.tick,
                resource_id: result.resource_id,
                nonce: result.nonce,
            });
            // Signaler qu'on arrête de miner
            let _ = tx_out.send(ClientMsg::Mining { resource_id: result.resource_id, on: false });
        }

        // Décider du prochain mouvement via la stratégie
        let maybe_move = {
            let state = shared_state.lock().unwrap();
            strategy.next_move(&*state)
        };
        if let Some((dx, dy)) = maybe_move {
            let _ = tx_out.send(ClientMsg::Move { dx, dy });
        }

        let tick = shared_state.lock().unwrap().tick;
        if tick > 0 {
            let _ = tx_out.send(ClientMsg::Heartbeat { tick });
        }

        // Pause
        thread::sleep(Duration::from_millis(200));
    }
}

// ─── Fonctions utilitaires ───────────────────────────────────────────────────

type WsStream = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

fn read_server_msg(ws: &mut WsStream) -> Option<ServerMsg> {
    match ws.read() {
        Ok(Message::Text(text)) => serde_json::from_str(&text).ok(),
        Ok(_) => None,
        Err(e) => {
            eprintln!("[!] Erreur WS lecture : {e}");
            None
        }
    }
}

fn send_client_msg(ws: &mut WsStream, msg: &ClientMsg) {
    let json = serde_json::to_string(msg).expect("sérialisation échouée");
    ws.send(Message::Text(json.into())).expect("envoi WS échoué");
}