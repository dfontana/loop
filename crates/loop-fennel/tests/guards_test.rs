//! Guard closures: transient/real routing, throwing guards, and non-boolean
//! returns.

mod common;

use loop_core::{CoreError, GuardEvaluator, Vars};
use serde_json::json;

#[test]
fn transient_vs_real_routing_is_mutually_exclusive() {
    let vm = common::vm();
    let config = common::default_config();
    let machine = vm
        .load_machine(&common::fixture("proj1487/machine.fnl"), &config)
        .expect("load_machine");

    let transient_guard = machine
        .edge("qa-staging", "qa-staging")
        .unwrap()
        .when
        .unwrap();
    let real_guard = machine.edge("qa-staging", "debug").unwrap().when.unwrap();
    let pass_guard = machine
        .edge("qa-staging", "validate-contract")
        .unwrap()
        .when
        .unwrap();

    let transient_vars =
        Vars::from_value(json!({"qa": {"result": "fail", "error_class": "transient"}}));
    assert!(vm.eval(transient_guard, &transient_vars).unwrap());
    assert!(!vm.eval(real_guard, &transient_vars).unwrap());
    assert!(!vm.eval(pass_guard, &transient_vars).unwrap());

    let real_vars = Vars::from_value(json!({"qa": {"result": "fail", "error_class": "http_5xx"}}));
    assert!(!vm.eval(transient_guard, &real_vars).unwrap());
    assert!(vm.eval(real_guard, &real_vars).unwrap());
    assert!(!vm.eval(pass_guard, &real_vars).unwrap());

    let pass_vars = Vars::from_value(json!({"qa": {"result": "pass"}}));
    assert!(!vm.eval(transient_guard, &pass_vars).unwrap());
    assert!(!vm.eval(real_guard, &pass_vars).unwrap());
    assert!(vm.eval(pass_guard, &pass_vars).unwrap());
}

#[test]
fn review_routing_guards() {
    let vm = common::vm();
    let config = common::default_config();
    let machine = vm
        .load_machine(&common::fixture("proj1487/machine.fnl"), &config)
        .expect("load_machine");

    let back_to_implement = machine.edge("review", "implement").unwrap().when.unwrap();
    let onward = machine.edge("review", "qa-staging").unwrap().when.unwrap();

    let changes_requested = Vars::from_value(json!({"review": {"result": "changes_requested"}}));
    assert!(vm.eval(back_to_implement, &changes_requested).unwrap());
    assert!(!vm.eval(onward, &changes_requested).unwrap());

    let clean = Vars::from_value(json!({"review": {"result": "clean"}}));
    assert!(!vm.eval(back_to_implement, &clean).unwrap());
    assert!(vm.eval(onward, &clean).unwrap());
}

#[test]
fn guard_that_throws_surfaces_as_guard_error() {
    let vm = common::vm();
    let config = common::default_config();
    let machine = vm
        .load_machine(&common::fixture("guard_throws.fnl"), &config)
        .expect("load_machine");
    let guard = machine.edge("a", "a").unwrap().when.unwrap();

    let err = vm.eval(guard, &Vars::new()).unwrap_err();
    match err {
        CoreError::Guard { detail, .. } => {
            assert!(detail.contains("boom"), "unexpected detail: {detail}");
        }
        other => panic!("expected CoreError::Guard, got {other:?}"),
    }
}

#[test]
fn guard_that_returns_non_boolean_is_a_guard_error_not_false() {
    let vm = common::vm();
    let config = common::default_config();
    let machine = vm
        .load_machine(&common::fixture("guard_nonboolean.fnl"), &config)
        .expect("load_machine");
    let guard = machine.edge("a", "a").unwrap().when.unwrap();

    let err = vm.eval(guard, &Vars::new()).unwrap_err();
    assert!(matches!(err, CoreError::Guard { .. }), "got {err:?}");
}

#[test]
fn guard_source_is_recorded_for_the_ledger() {
    let vm = common::vm();
    let config = common::default_config();
    let machine = vm
        .load_machine(&common::fixture("proj1487/machine.fnl"), &config)
        .expect("load_machine");

    let guard = machine
        .edge("qa-staging", "qa-staging")
        .unwrap()
        .when
        .unwrap();
    assert!(vm.source(guard).is_some());
}

#[test]
fn loop_runtime_module_helpers_work() {
    let vm = common::vm();
    let value = vm
        .eval_file(&common::fixture("loop_module.fnl"))
        .expect("eval_file");
    let table = match value {
        mlua::Value::Table(t) => t,
        other => panic!("expected a table, got {other:?}"),
    };
    assert!(table.get::<bool>("transient").unwrap());
    assert!(!table.get::<bool>("real-on-transient").unwrap());
    assert!(table.get::<bool>("real-on-real").unwrap());
    assert_eq!(table.get::<f64>("mins").unwrap(), 120.0);
    assert_eq!(table.get::<f64>("secs").unwrap(), 45.0);
}
