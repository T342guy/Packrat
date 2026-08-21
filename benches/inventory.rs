// SPDX-License-Identifier: GPL-3.0-only
//! Benchmarks over a synthetic inventory much larger than a real garage.
//!
//! These exist because a real regression slipped through once: opening a shelf
//! re-queried every box's contents separately, which was invisible on the
//! example data and 16x slower on a full one. The shelf case is benchmarked
//! deliberately.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use packrat::models::{ContainerInput, ItemInput};
use packrat::store::{self, ItemQuery};
use rusqlite::Connection;
use std::hint::black_box;

/// Containers, items, and one deliberately crowded shelf.
struct Fixture {
    conn: Connection,
    crowded_shelf: i64,
    a_box: i64,
    barcode: String,
}

fn build(containers_per_shelf: usize, items: usize) -> Fixture {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    packrat::db::migrate(&mut conn).unwrap();

    let make = |conn: &Connection, name: &str, kind: &str, parent: Option<i64>| -> i64 {
        store::create_container(
            conn,
            &ContainerInput {
                name: name.to_string(),
                kind: kind.to_string(),
                parent_id: parent,
                notes: String::new(),
                photo_id: None,
                code: None,
                barcode: None,
            },
        )
        .unwrap()
        .id
    };

    let garage = make(&conn, "Garage", "area", None);
    let mut boxes = Vec::new();
    for shelf_index in 0..8 {
        let shelf = make(
            &conn,
            &format!("Shelf {shelf_index}"),
            "shelf",
            Some(garage),
        );
        for box_index in 0..containers_per_shelf {
            boxes.push(make(
                &conn,
                &format!("Box {shelf_index}-{box_index}"),
                "box",
                Some(shelf),
            ));
        }
    }
    // The pathological page: one shelf holding far more than the rest.
    let crowded_shelf = make(&conn, "Crowded shelf", "shelf", Some(garage));
    for box_index in 0..40 {
        boxes.push(make(
            &conn,
            &format!("Crowded {box_index}"),
            "box",
            Some(crowded_shelf),
        ));
    }

    let words = [
        "drill", "saw", "hammer", "wrench", "bolt", "screw", "paint", "rope", "tarp", "tent",
        "lantern", "cable", "filter", "blade", "sander", "clamp",
    ];
    for index in 0..items {
        let name = format!(
            "{} {} {index}",
            words[index % words.len()],
            words[(index * 7) % words.len()]
        );
        store::create_item(
            &mut conn,
            &ItemInput {
                name,
                description: words.join(" "),
                quantity: (index % 9) as i64 + 1,
                container_id: Some(boxes[index % boxes.len()]),
                photo_id: None,
                tags: vec![words[index % words.len()].to_string()],
                barcode: Some(format!("{:013}", 4_000_000_000_000u64 + index as u64)),
            },
        )
        .unwrap();
    }

    Fixture {
        conn,
        crowded_shelf,
        a_box: boxes[0],
        barcode: format!("{:013}", 4_000_000_000_000u64 + 1),
    }
}

fn benchmarks(c: &mut Criterion) {
    // A very full garage, then an implausible one, to show how each query
    // scales rather than just how fast it is on one dataset.
    for items in [1_000usize, 4_000, 16_000] {
        let fixture = build(8, items);
        let mut group = c.benchmark_group(format!("inventory/{items}-items"));
        group.sample_size(20);

        group.bench_function("search one term", |b| {
            b.iter(|| {
                store::query_items(
                    &fixture.conn,
                    &ItemQuery {
                        q: Some("drill".into()),
                        ..Default::default()
                    },
                )
                .unwrap()
            })
        });

        group.bench_function("search two terms", |b| {
            b.iter(|| {
                store::query_items(
                    &fixture.conn,
                    &ItemQuery {
                        q: Some("drill saw".into()),
                        ..Default::default()
                    },
                )
                .unwrap()
            })
        });

        group.bench_function("all containers", |b| {
            b.iter(|| store::all_containers(&fixture.conn).unwrap())
        });

        group.bench_function("open a box", |b| {
            b.iter(|| store::container_detail(&fixture.conn, black_box(fixture.a_box)).unwrap())
        });

        // The regression case: a shelf with forty boxes on it.
        group.bench_function("open a crowded shelf", |b| {
            b.iter(|| {
                store::container_detail(&fixture.conn, black_box(fixture.crowded_shelf)).unwrap()
            })
        });

        group.bench_function("scan a barcode", |b| {
            b.iter(|| store::resolve_scan(&fixture.conn, black_box(&fixture.barcode)).unwrap())
        });

        group.bench_function("stats", |b| b.iter(|| store::stats(&fixture.conn).unwrap()));

        group.bench_with_input(
            BenchmarkId::new("list every item", items),
            &items,
            |b, _| b.iter(|| store::query_items(&fixture.conn, &ItemQuery::default()).unwrap()),
        );

        group.finish();
    }
}

criterion_group!(inventory, benchmarks);
criterion_main!(inventory);
