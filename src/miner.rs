// ─── Partie 2 : Pool de mineurs ──────────────────────────────────────────────
//
// Objectif : créer un pool de N threads qui cherchent des nonces en parallèle.
//
// Concepts exercés : thread::spawn, mpsc::channel, Arc, move closures.
//
// Architecture :
//
//   Thread principal                        Threads mineurs (x N)
//        |                                        |
//        |── mpsc::Sender<MineRequest> ──────────>|  (challenges à résoudre)
//        |                                        |
//        |<── mpsc::Sender<MineResult> ──────────>|  (solutions trouvées)
//        |                                        |
//
// Chaque thread mineur :
//   1. Attend un MineRequest sur son channel
//   2. Appelle pow::pow_search() avec un start_nonce différent
//   3. Si un nonce est trouvé, envoie un MineResult
//
// ─────────────────────────────────────────────────────────────────────────────

use uuid::Uuid;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use rand::random;

use crate::pow::pow_search;

/// Requête de minage envoyée aux threads mineurs.
#[derive(Debug, Clone)]
pub struct MineRequest {
    pub seed: String,
    pub tick: u64,
    pub resource_id: Uuid,
    pub agent_id: Uuid,
    pub target_bits: u8,
}

/// Résultat renvoyé par un mineur quand il trouve un nonce valide.
#[derive(Debug, Clone)]
pub struct MineResult {
    pub tick: u64,
    pub resource_id: Uuid,
    pub nonce: u64,
}

// TODO: Définir la structure MinerPool.
//
// Elle doit contenir :
//   - Le Sender pour envoyer des MineRequest aux threads
//   - Le Receiver pour récupérer les MineResult
//
// Indice : les types sont :
//   std::sync::mpsc::Sender<MineRequest>
//   std::sync::mpsc::Receiver<MineResult>
//
pub struct MinerPool {
    pub sender: Sender<MineRequest>,
    pub receiver: Receiver<MineResult>,
}

// TODO: Implémenter MinerPool.
//
impl MinerPool {
    /// Crée un pool de `n` threads mineurs.
    ///
    /// Chaque thread :
    ///   1. Possède un Receiver<MineRequest> (partagé via Arc<Mutex<>>)
    ///   2. Possède un Sender<MineResult> (cloné pour chaque thread)
    ///   3. Boucle : recv() → pow_search() → send() si trouvé
    ///
    /// Indices :
    ///   - Un seul Receiver existe par channel. Pour le partager entre N threads,
    ///     il faut le wrapper dans Arc<Mutex<Receiver<MineRequest>>>.
    ///   - Chaque thread clone le Arc pour accéder au Receiver.
    ///   - pow::pow_search() prend un start_nonce et un batch_size.
    ///     Utilisez rand::random::<u64>() comme start_nonce pour que chaque
    ///     appel explore une zone différente.
    ///   - Batch size recommandé : 100_000
    ///
    pub fn new(n: usize) -> Self {
        // Créer les 2 channels :
        //   - (request_tx, request_rx) pour envoyer les challenges
        //   - (result_tx, result_rx) pour recevoir les solutions
        let (request_tx, request_rx) = channel::<MineRequest>();
        let (result_tx, result_rx) = channel::<MineResult>();
        //
        // Wrapper request_rx dans Arc<Mutex<>>
        let shared_rx = Arc::new(Mutex::new(request_rx));
        //
        // Pour chaque thread (0..n) :
        //   - Cloner le Arc<Mutex<Receiver<MineRequest>>>
        //   - Cloner le result_tx
        //   - thread::spawn(move || { ... boucle de minage ... })
        for _ in 0..n {
            let rx_clone = Arc::clone(&shared_rx);
            let tx_clone = result_tx.clone();

            thread::spawn(move || {
                // Boucle de minage 
                loop {
                    // Réception de la request
                    let request = {
                        let rx_lock = rx_clone.lock().unwrap();
                        match rx_lock.recv() {
                            Ok(request) => request,
                            Err(_) => break,
                        }
                    };

                    let start_nonce = random::<u64>();
                    let batch_size = 100_000;

                    // Recherche d'un nonce
                    let found_nonce = pow_search(
                        &request.seed, 
                        request.tick, 
                        request.resource_id, 
                        request.agent_id, 
                        request.target_bits, 
                        start_nonce, 
                        batch_size
                    );

                    // Envoi du nonce trouvé
                    if let Some(nonce) = found_nonce {
                        let result = MineResult {
                            tick: request.tick,
                            resource_id: request.resource_id,
                            nonce: nonce,
                        };
                        let _ = tx_clone.send(result);
                    }
                }
            });
        }
        //
        // Retourner MinerPool { request_tx, result_rx }
        MinerPool { 
            sender: request_tx,
            receiver: result_rx,
        }        
    }

    /// Envoie un challenge de minage au pool.
    pub fn submit(&self, request: MineRequest) {
        let _ = self.sender.send(request);  // Gérer cas d'erreur
    }

    /// Tente de récupérer un résultat sans bloquer.
    pub fn try_recv(&self) -> Option<MineResult> {
        self.receiver.try_recv().ok()
    }
}
