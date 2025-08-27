use {
    qat_shim::qat::{self, Instance},
    solana_entry::entry,
    solana_ledger::{
        blockstore::{self, make_many_slot_entries, test_all_empty_or_min, Blockstore},
        get_tmp_ledger_path_auto_delete,
    },
    solana_sdk::hash::Hash,
    std::{
        sync::{mpsc::channel, Arc},
        thread::Builder,
    },
};

fn setup_qat() -> (
    Option<std::sync::mpsc::Sender<()>>,
    Option<std::thread::JoinHandle<()>>,
    Instance,
) {
    qat_shim::qat::start_session("SSL").expect("start session failed");
    qat_shim::qat::qae_mem_init().expect("qae_mem_init failed");
    let inst: Instance = qat::get_first_instance().expect("failed to get first instance");
    inst.set_address_translation()
        .expect("set address translation failed");
    inst.start().expect("start instance failed");
    let (tx_poll, poll) = if inst.is_polled().unwrap() {
        let (tx, rx) = channel();
        let inst2 = inst.clone();
        let poll = std::thread::spawn(move || {
            while matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)) {
                let _ = inst2.clone().poll_once();
            }
            println!("Polling thread exiting");
        });
        (Some(tx), Some(poll))
    } else {
        (None, None)
    };
    (tx_poll, poll, inst)
}

fn qat_tear_down(
    tx_poll: Option<std::sync::mpsc::Sender<()>>,
    poll: Option<std::thread::JoinHandle<()>>,
    inst: Instance,
) {
    if let Some(tx_poll) = tx_poll {
        tx_poll
            .send(())
            .expect("Failed to send stop signal to polling thread");
        poll.unwrap().join().expect("Polling thread panicked");
    }
    inst.stop().expect("stop instance failed");
    qat_shim::qat::stop_session().expect("stop session failed");
    qat_shim::qat::qae_mem_destroy();
}

#[test]
fn test_multiple_threads_insert_shred() {
    let (tx_poll, poll, inst) = setup_qat();
    let ledger_path = get_tmp_ledger_path_auto_delete!();
    let blockstore = Arc::new(Blockstore::open(ledger_path.path()).unwrap());

    for _ in 0..100 {
        let num_threads = 10;

        // Create `num_threads` different ticks in slots 1..num_threads + 1, all
        // with parent = slot 0
        let threads: Vec<_> = (0..num_threads)
            .map(|i| {
                let entries = entry::create_ticks(1, 0, Hash::default());
                let shreds = blockstore::entries_to_test_shreds(
                    &entries,
                    i + 1,
                    0,
                    false,
                    0,
                    true, // merkle_variant
                );
                let blockstore_ = blockstore.clone();
                Builder::new()
                    .name("blockstore-writer".to_string())
                    .spawn(move || {
                        blockstore_.insert_shreds(shreds, None, false).unwrap();
                    })
                    .unwrap()
            })
            .collect();

        for t in threads {
            t.join().unwrap()
        }

        // Check slot 0 has the correct children
        let mut meta0 = blockstore.meta(0).unwrap().unwrap();
        meta0.next_slots.sort_unstable();
        let expected_next_slots: Vec<_> = (1..num_threads + 1).collect();
        assert_eq!(meta0.next_slots, expected_next_slots);

        // Delete slots for next iteration
        blockstore.purge_and_compact_slots(0, num_threads + 1);
    }
    qat_tear_down(tx_poll, poll, inst);
}

#[test]
fn test_purge_huge() {
    let (tx_poll, poll, inst) = setup_qat();
    let ledger_path = get_tmp_ledger_path_auto_delete!();
    let blockstore = Blockstore::open(ledger_path.path()).unwrap();

    let (shreds, _) = make_many_slot_entries(0, 5000, 10);
    blockstore.insert_shreds(shreds, None, false).unwrap();

    blockstore.purge_and_compact_slots(0, 4999);
    test_all_empty_or_min(&blockstore, 5000);
    qat_tear_down(tx_poll, poll, inst);
}
