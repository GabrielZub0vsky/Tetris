//! Simple unit tests for the server
#![cfg(feature = "basic_tests")]

use bevy::prelude::*;
use lightyear::prelude::Lifetime;
use std::collections::HashSet;

use super::*;

mod end_to_end;
mod multiplayer;
mod net;
mod rr;

#[derive(Component, Debug, PartialEq, Clone, Copy)]
struct Foo;

#[derive(Component, Debug, Eq, PartialEq, Clone, Copy, Hash, PartialOrd, Ord)]
struct Bar(u32);

#[derive(Resource, Copy, Clone)]
struct Owners {
    owner1: Entity,
    owner2: Entity,
}

#[test]
fn controlled_by_test() {
    let owner = Entity::from_raw_u32(67).unwrap();

    assert_eq!(
        controlled_by(owner),
        ControlledBy {
            owner,
            lifetime: Lifetime::SessionBased,
        }
    );
}

fn make_entities(mut commands: Commands) {
    let owner1 = commands.spawn_empty().id();
    let owner2 = commands.spawn_empty().id();

    commands.spawn((Foo, controlled_by(owner1)));
    commands.spawn(((Foo, Bar(7)), controlled_by(owner2)));
    commands.spawn((Bar(1), controlled_by(owner1)));
    commands.spawn((Bar(2), controlled_by(owner1)));
    commands.spawn((Bar(2), controlled_by(owner2)));
    commands.insert_resource(Owners { owner1, owner2 });
}

fn test_build_fns(
    query1: Query<(&Foo, &ControlledBy)>,
    query2: Query<(&Foo, &ControlledBy), With<Bar>>,
    query3: Query<(&Bar, &ControlledBy)>,
    owners: Res<Owners>,
) {
    let table1 = build_per_client_table(query1);
    let table2 = build_per_client_table(query2);
    let list1 = build_per_client_lists(query1);
    let list2 = build_per_client_lists(query2);
    let list3 = build_per_client_lists(query3)
        .into_iter()
        .map(|(k, mut v)| {
            v.sort();
            (k, v)
        })
        .collect::<HashMap<_, _>>();
    let Owners { owner1, owner2 } = *owners;

    assert_eq!(table1, HashMap::from([(owner1, &Foo), (owner2, &Foo)]));
    assert_eq!(table2, HashMap::from([(owner2, &Foo)]));
    assert_eq!(
        list1,
        HashMap::from([(owner1, vec![&Foo]), (owner2, vec![&Foo])])
    );
    assert_eq!(list2, HashMap::from([(owner2, vec![&Foo])]));
    assert_eq!(
        list3,
        HashMap::from([(owner1, vec![&Bar(1)]), (owner2, vec![&Bar(2), &Bar(7)])])
    );
}

fn test_take(
    query1: Query<(&Foo, &ControlledBy)>,
    query2: Query<(&Foo, &ControlledBy), With<Bar>>,
    owners: Res<Owners>,
) {
    assert_eq!(
        take_controlled_by(query1, owners.owner1).copied(),
        Some(Foo)
    );
    assert_eq!(
        take_controlled_by(query1, owners.owner2).copied(),
        Some(Foo)
    );
    assert_eq!(take_controlled_by(query2, owners.owner1).copied(), None);
    assert_eq!(
        take_controlled_by(query2, owners.owner2).copied(),
        Some(Foo)
    );
}

fn test_filter(query: Query<(&Bar, &ControlledBy)>, owners: Res<Owners>) {
    assert_eq!(
        filter_controlled_by(query, owners.owner1)
            .copied()
            .collect::<HashSet<_>>(),
        HashSet::from([Bar(1)]),
    );
    assert_eq!(
        filter_controlled_by(query, owners.owner1)
            .copied()
            .collect::<HashSet<_>>(),
        HashSet::from([Bar(2), Bar(7)]),
    );
}

#[test]
fn owner_filters() {
    let owner = Entity::from_raw_u32(67).unwrap();

    assert_eq!(
        controlled_by(owner),
        ControlledBy {
            owner,
            lifetime: Lifetime::SessionBased,
        }
    );

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_systems(Startup, make_entities)
        .add_systems(Update, (test_build_fns, test_take, test_filter));
}
