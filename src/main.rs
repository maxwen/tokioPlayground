use std::time::Duration;

use tokio::sync::broadcast;
use tokio::time::sleep;

async fn task_loop_one(tx: broadcast::Sender<i32>) {
    let mut count: i32 = 0;
    loop {
        println!("task_loop_one");
        tx.send(count).unwrap();
        count += 1;
        sleep(Duration::from_millis(200)).await;
    }
}

async fn task_loop_two<'a>(mut rx: broadcast::Receiver<i32>) {
    loop {
        match rx.recv().await {
            Ok(value) => {
                println!("task_loop_two receiver {}", value);
            }
            Err(error) => { break; }
        }
    }
}

async fn task_slow() {
    println!("task_slow start");
    sleep(Duration::from_millis(2000)).await;
    println!("task_slow stop");
}

#[tokio::main]
/*
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            main() ...
        })
 */
async fn main() {
    println!("Hello, world!");

    let (tx, rx) = broadcast::channel(16);

    tokio::spawn(task_loop_two(rx));
    tokio::spawn(task_loop_one(tx.clone()));

    let t = tokio::spawn(async move {
        task_slow().await
    });

    let mut rx1 = tx.subscribe();
    loop {
        // sleep(Duration::from_millis(2000)).await;
        match rx1.recv().await {
            Ok(value) => {
                println!("main receiver {}", value);
            }
            Err(error) => { break; }
        }
    }
    t.await.unwrap();
}
