# Level-triggered gates

Scenarios for declaring gates, running and checking them, changing parameters, and operating on gates and the resources they hold.
A ticked case names the automated test that covers it.

## Declaring a gate

- [x] A gate is declared by name, and the same name is the same gate (verifies spec: GATE#declaration) [`a_gate_is_declared_by_name_and_the_same_name_is_the_same_gate`]
- [x] A gate name that is not a valid name throws (verifies spec: GATE#declaration) [`a_gate_name_that_is_not_a_valid_name_throws`]
- [x] Declaring a gate in an action closure throws, as does every builder method outside the static context (verifies spec: GATE#declaration) [`declaring_a_gate_in_an_action_closure_throws`, `every_builder_method_throws_outside_the_static_context`]
- [x] A gate is not a resource, and a runtime handle method given a gate as a selection throws (verifies spec: GATE#declaration) [`a_gate_is_not_a_resource`, `a_gate_is_not_a_selection_for_a_runtime_handle_method`]

## Keys

- [x] A gate with no key has the unit key, and calling `key` again replaces the key (verifies spec: GATE#keys) [`a_gate_without_a_key_has_the_unit_key_and_calling_key_again_replaces_it`]
- [x] A key that is not representable in JSON throws (verifies spec: GATE#keys) [`a_key_that_is_not_representable_in_json_throws`]
- [x] Maps are the same key regardless of member order, and an integer and a float are never the same key (verifies spec: GATE#keys) [`two_keys_are_the_same_when_they_are_the_same_json_value`, `two_keys_that_differ_in_value_order_or_number_type_are_not_the_same`]
- [ ] A gate keyed on a parameter's value moves when the value moves through a stored value, a default, a derived value or a new definition (verifies spec: GATE#keys)

## Body and `last`

- [x] A body is set once and must be a closure (verifies spec: GATE#body) [`a_gate_takes_a_body_once_and_only_a_closure`]
- [x] The body is called with as many arguments as it declares, and one declaring more than two fails when called (verifies spec: GATE#body) [`a_gate_body_is_called_with_as_many_arguments_as_it_declares`, `a_gate_body_declaring_more_than_two_parameters_fails_when_called`]
- [x] `last` is the app as of the last success, and the unit value where there is none (verifies spec: GATE#body) [`last_is_the_app_at_the_last_success_and_the_unit_value_where_there_is_none`]
- [ ] `last` evaluates with the parameter values of the generation of the last success, not the current ones (verifies spec: GATE#body)
- [ ] `last.file` and `last.dir` read the bundle of the generation of the last success (verifies spec: GATE#body)
- [ ] Calling a resource method on `last` does not affect the app (verifies spec: GATE#body)
- [ ] A version that moves twice while a migration keeps failing leaves `last` at the release last migrated (verifies spec: GATE#body)
- [ ] A gate without a body is satisfied once its check passes (verifies spec: GATE#body)
- [x] A resource the body starts that is ordered after the gate starts without waiting for the gate (verifies spec: GATE#body) [`rt_start_leaves_out_what_a_gate_holds_and_what_waits_on_it_except_in_that_gates_body`]

## Check

- [x] A check is a named Job with a deadline, with an optional interval of at least one second, and anything else throws (verifies spec: GATE#check) [`a_check_is_a_named_job_with_a_deadline_and_an_optional_interval`, `a_check_that_is_not_a_named_job_with_a_deadline_throws`, `a_check_takes_only_an_interval_of_at_least_a_second`]
- [x] Calling `check` again replaces the check (verifies spec: GATE#check) [`calling_check_again_replaces_the_check`]
- [x] A Job is the check of one gate only (verifies spec: GATE#check) [`a_job_checks_one_gate`]
- [x] A check Job in its gate's exclusive selection, or ordered after its gate directly or through other targets, fails evaluation (verifies spec: GATE#check) [`a_check_job_in_its_gates_exclusive_selection_fails_evaluation`, `a_check_job_ordered_after_its_gate_fails_evaluation_directly_or_through_other_targets`]
- [x] A check passes on exit status 0, and fails on any other status or at its deadline (verifies spec: GATE#check) [`a_check_passes_when_its_job_exits_zero_and_fails_otherwise`, `a_check_is_judged_by_how_its_job_exited_and_by_its_deadline`]

## Exclusive selection

- [x] The selection resolves to the Deployments and Jobs of the app once evaluation completes, and a later call replaces it (verifies spec: GATE#exclusive-selection) [`an_exclusive_selection_resolves_to_the_deployments_and_jobs_of_the_app_at_the_end`, `calling_exclusive_again_replaces_the_selection`]
- [x] A value that is not a selection makes `exclusive` throw (verifies spec: GATE#exclusive-selection) [`a_value_that_is_not_a_selection_makes_exclusive_throw`]
- [ ] A selection holding a dynamic resource makes `exclusive` throw (verifies spec: GATE#exclusive-selection)
- [x] A selection holding one of the gate's targets, directly or through other targets, fails evaluation (verifies spec: GATE#exclusive-selection) [`an_exclusive_selection_holding_a_target_fails_evaluation_directly_or_through_other_targets`]

## Start order

- [x] `after` orders a gate or workload after gates, named Deployments and Jobs, adds to earlier calls, and returns its builder (verifies spec: GATE#start-order) [`after_orders_a_workload_or_gate_after_gates_deployments_and_jobs`, `each_after_call_adds_to_the_targets_already_given`, `after_returns_the_builder_it_was_called_on`]
- [x] A target that is not a gate, a named Deployment or a named Job throws, and `after` throws in the action context (verifies spec: GATE#start-order) [`a_target_that_is_not_a_gate_a_named_deployment_or_a_named_job_throws`, `after_throws_in_the_action_context_and_on_an_anonymous_pod`]
- [x] Ordering after oneself, or in a cycle, fails evaluation (verifies spec: GATE#start-order) [`a_gate_or_workload_ordered_after_itself_fails_evaluation`, `ordering_that_forms_a_cycle_fails_evaluation_naming_it`]
- [x] A Job target is ready by the outcome of its most recent instance (verifies spec: GATE#start-order) [`a_job_target_is_ready_by_its_most_recent_instances_outcome`]
- [x] A dependant waits on the targets that are not ready (verifies spec: GATE#start-order) [`a_dependant_waits_on_the_targets_that_are_not_ready_and_is_told_which`, `a_gate_is_not_run_while_a_target_is_not_ready`]
- [ ] A stopped Deployment or Job, and a Deployment at an effective scale of zero, are not ready (verifies spec: GATE#start-order)
- [ ] A running instance is not stopped because a target stops being ready (verifies spec: GATE#start-order)
- [x] A workload ordered directly after a gate takes the gate's key in its spec hash (verifies spec: GATE#start-order) [`the_spec_hash_moves_with_the_key_of_a_gate_the_workload_is_ordered_after`]
- [ ] A running Deployment ordered after a gate is replaced by its update strategy once the gate is satisfied for a new key (verifies spec: GATE#start-order)

## Gate state

- [x] A gate with no record is unsatisfied, and a valid success satisfies the gate for its key and no other (verifies spec: GRUN#gate-state) [`a_gate_with_no_record_is_unsatisfied`, `a_gate_is_satisfied_for_the_key_of_its_valid_success_and_no_other`]
- [x] A digest taken under another secret key does not satisfy the gate (verifies spec: GRUN#outcome) [`a_gate_keyed_the_same_by_another_secret_key_is_unsatisfied`]
- [x] An unreadable record leaves the gate undetermined, not unsatisfied (verifies spec: GRUN#gate-state) [`an_unreadable_record_leaves_the_gate_undetermined_and_not_unsatisfied`, `a_gate_record_table_that_cannot_be_queried_leaves_every_gate_undetermined`]
- [x] A definition that fails to evaluate leaves the gates undetermined (verifies spec: GRUN#gate-state) [`a_definition_that_fails_to_evaluate_leaves_the_gates_undetermined`]
- [x] A valid success whose check could not be run is undetermined (verifies spec: GRUN#gate-state) [`a_valid_success_whose_check_could_not_be_run_is_undetermined`]
- [x] An undetermined gate is not run, and a `gate_state_unreadable` fault stands while its record cannot be read (verifies spec: GRUN#gate-state) [`a_gate_whose_record_cannot_be_read_is_not_run_and_a_fault_says_so`]
- [ ] An undetermined gate starts no dependant and stops nothing (verifies spec: GRUN#gate-state)

## Initiating gate runs

- [x] An unsatisfied gate is run by the reconciliation loop (verifies spec: GRUN#initiating-runs) [`an_unsatisfied_gate_is_run_by_the_loop_and_the_app_comes_to_be_set_up`]
- [x] A gate waiting out its back-off, a paused gate, and a gate after one that is not ready are not run (verifies spec: GRUN#initiating-runs) [`a_gate_waiting_out_its_back_off_a_paused_gate_and_a_gate_after_one_are_not_run`]
- [x] Due gates run one at a time in declaration order (verifies spec: GRUN#initiating-runs) [`due_gates_run_one_at_a_time_in_the_order_the_script_declared_them`, `gates_come_in_the_order_the_script_declared_them`]
- [x] A run the concurrency rules do not admit is retried later and has not failed (verifies spec: GRUN#initiating-runs) [`a_run_the_concurrency_rules_do_not_admit_is_retried_and_has_not_failed`]
- [ ] A gate run has the trigger `gate` and runs against the generation current when it is dispatched (verifies spec: GRUN#initiating-runs)
- [ ] A change that moves a gate's key leaves it unsatisfied and runs it with no other operation requested (verifies spec: OPS#changes-to-an-app)

## Preparing a run

- [x] A run starts by invalidating the last success, and keeps the last success's generation for `last` (verifies spec: GRUN#preparation) [`a_run_starts_by_invalidating_the_last_success_and_keeps_its_generation_for_last`]
- [x] A run that cannot invalidate the last success fails with nothing stopped (verifies spec: GRUN#preparation) [`a_run_that_cannot_invalidate_the_last_success_fails_before_anything_else_is_done`]
- [x] A run pulls the images of its body, its check and its dependants, and no others (verifies spec: GRUN#preparation) [`a_run_pulls_the_images_of_its_body_its_check_and_its_dependants_and_no_others`]
- [x] A failed pull fails the run with nothing stopped (verifies spec: GRUN#preparation) [`a_run_whose_images_cannot_be_pulled_fails_with_nothing_stopped`]
- [ ] The run waits until every instance of the exclusive selection is Terminated before the body runs (verifies spec: GRUN#preparation)
- [ ] The instances that were running keep running when a pull fails (verifies spec: GRUN#preparation)

## Outcome

- [x] A successful run records the digest of its key and its generation, and releases its hold (verifies spec: GRUN#outcome) [`a_run_that_succeeds_records_the_digest_of_its_key_and_its_generation_and_releases_its_hold`]
- [x] A run succeeds only once its check passes (verifies spec: GRUN#outcome) [`a_run_succeeds_only_once_its_check_passes`]
- [x] A run whose success cannot be recorded counts as failed (verifies spec: GRUN#outcome) [`a_run_whose_success_cannot_be_recorded_counts_as_failed_and_keeps_its_hold`]
- [x] A run whose check cannot be run does not succeed and files `gate_check_unavailable` (verifies spec: GRUN#outcome) [`a_run_whose_check_cannot_be_run_does_not_succeed_and_files_gate_check_unavailable`]
- [ ] A run whose body throws does not succeed (verifies spec: GRUN#outcome)
- [ ] A gate's record holds no key and nothing from which the key could be recovered without the secret key (verifies spec: GRUN#outcome)
- [ ] A run for an old key leaves the gate unsatisfied when the key has moved by the end, and another run follows (verifies spec: GRUN#outcome)
- [ ] An interrupted gate run that is not replayed leaves the gate unsatisfied, and it runs again (verifies spec: GRUN#outcome)

## Retries and pauses

- [x] The delay between failed runs doubles from thirty seconds to an hour (verifies spec: GRUN#retries) [`the_delay_between_failed_runs_doubles_from_thirty_seconds_to_an_hour`]
- [x] A failed run is retried once its delay has passed and not before, and a gate failing for days is run again after the cap (verifies spec: GRUN#retries) [`a_failed_run_is_retried_once_its_delay_has_passed_and_not_before`, `a_gate_that_has_failed_for_days_is_run_again_after_the_cap`]
- [x] A success resets the delay, and a move of the key makes the next run due at once (verifies spec: GRUN#retries) [`consecutive_failed_runs_for_one_key_count_up_and_a_success_resets_them`, `a_move_of_the_key_makes_the_next_run_due_at_once_and_restarts_the_delay`]
- [x] The back-off holds across a restart of the runtime (verifies spec: GRUN#retries) [`back_off_holds_across_a_restart_of_the_runtime`]
- [x] `gate_failed` stands while the gate is unsatisfied after a failed run and clears once it is satisfied (verifies spec: GRUN#retries) [`gate_failed_stands_while_the_gate_is_unsatisfied_after_a_failed_run_and_clears_when_it_is_satisfied`, `a_gate_failed_fault_survives_what_leaves_the_gate_undetermined`]
- [x] A cancelled run pauses the gate for its key, the pause holds across a restart, and it ends when an operator runs the gate or the key moves (verifies spec: GRUN#pauses) [`a_cancelled_run_pauses_the_gate_for_its_key_and_files_gate_failed`, `a_pause_holds_across_a_restart_until_an_operator_runs_the_gate_or_the_key_moves`]

## Checks

- [x] A check that passes leaves the gate satisfied and clears its dependants to start (verifies spec: GRUN#checks) [`a_check_that_passes_leaves_the_gate_satisfied_and_clears_its_dependants_to_start`]
- [x] A check that fails invalidates the last success, so the gate runs again (verifies spec: GRUN#checks) [`a_check_that_fails_invalidates_the_last_success_so_the_gate_is_run_again`]
- [x] A check with an interval runs every interval while no run is in progress (verifies spec: GRUN#checks) [`a_check_with_an_interval_runs_every_interval_while_no_run_is_in_progress`]
- [x] A dependant with no running instance starts only once a check has passed (verifies spec: GRUN#checks) [`a_gate_with_a_check_clears_a_dependant_to_start_from_stopped_only_once_a_check_passed`]
- [x] A check that cannot be run leaves the gate undetermined, holds `gate_check_unavailable`, and is retried after its delay across a restart (verifies spec: GRUN#checks) [`a_check_that_cannot_be_run_leaves_the_gate_undetermined_and_is_retried_after_its_delay_across_a_restart`, `gate_check_unavailable_stands_until_a_check_runs_to_an_outcome`]
- [ ] A check runs whatever other lifecycle operation is in progress (verifies spec: GRUN#checks)
- [ ] The Job of a check is defined as of the run's generation for the check that ends a run, and as of the current generation otherwise (verifies spec: GRUN#checks)

## Cancellation

- [x] Cancelling while the check runs ends the run at once (verifies spec: GRUN#cancellation) [`cancelling_while_a_check_runs_ends_it_at_once`]
- [ ] Cancelling while images are pulled, or while the exclusive selection is stopping, ends the run at once (verifies spec: GRUN#cancellation)

## Gates no longer declared, set up and uninstall

- [x] A gate no longer declared loses its queued run, faults and hold, and keeps its record (verifies spec: GRUN#gates-no-longer-declared) [`a_gate_the_definition_no_longer_declares_loses_its_faults_and_its_hold_but_keeps_its_record`, `a_queued_run_of_a_gate_the_definition_no_longer_declares_is_withdrawn`]
- [ ] A gate declared again under the same name resumes from its record (verifies spec: GRUN#gates-no-longer-declared)
- [x] An app is marked set up the first time every gate is satisfied at once, and stays so (verifies spec: GRUN#app-setup) [`an_app_is_set_up_once_every_gate_is_satisfied_at_once`, `an_app_is_marked_set_up_the_first_time_every_gate_is_satisfied_and_stays_so`]
- [x] An app that cannot be judged is not marked set up (verifies spec: GRUN#app-setup) [`an_app_that_cannot_be_judged_is_not_marked_set_up`]
- [x] An app with no gates is set up once installed (verifies spec: GRUN#app-setup) [`an_app_that_declares_no_gates_is_not_installing_once_installed`]
- [x] Uninstall deletes the gate records, the holds and the setup record of that app alone (verifies spec: GRUN#uninstall) [`uninstall_deletes_the_records_the_holds_and_the_setup_of_the_app_alone`, `a_gate_record_is_unaffected_by_the_resources_of_other_apps`]
- [x] While the records cannot be deleted the app does not move to `NotInstalled` (verifies spec: GRUN#uninstall) [`an_uninstall_deletes_the_state_of_the_gates_and_waits_while_it_cannot`]
- [ ] Installing again after an uninstall runs every gate as for the first time (verifies spec: GRUN#uninstall)

## Holds

- [x] A resource a gate holds is desired Unscheduled whatever its scale, and a Job likewise (verifies spec: GATE#exclusive-selection) [`a_resource_a_gate_holds_is_desired_unscheduled_whatever_its_scale`, `a_job_a_gate_holds_is_desired_unscheduled`]
- [x] A hold is cleared once its gate is satisfied and not before, and survives a restart of the runtime (verifies spec: GATE#exclusive-selection) [`a_hold_is_cleared_once_its_gate_is_satisfied_and_not_before`, `a_hold_stays_held_across_a_restart_of_the_runtime`]
- [x] A hold that cannot be read is an error and not an empty set (verifies spec: GATE#exclusive-selection) [`holds_that_cannot_be_read_are_an_error_and_not_an_empty_set`]
- [x] `rt.start` and `rt.stop` leave out what gates hold and what waits on them, except in that gate's own body (verifies spec: RT#resources-a-gate-holds) [`rt_start_leaves_out_what_a_gate_holds_and_what_waits_on_it_except_in_that_gates_body`, `rt_stop_leaves_out_what_a_gate_holds_and_what_waits_on_it`]
- [x] `rt.restart` of a Deployment a gate holds throws (verifies spec: RT#resources-a-gate-holds) [`rt_restart_of_a_deployment_a_gate_holds_or_that_waits_on_a_gate_throws`]
- [x] A call that would act on a workload fails while what the gates hold cannot be read (verifies spec: RT#resources-a-gate-holds) [`a_call_that_would_act_on_a_workload_fails_while_what_gates_hold_cannot_be_read`]

## Lifecycle operations

- [x] A gate run is persisted before it starts and read back as a gate run, and a queued gate run is restored in order (verifies spec: OPS#concurrency) [`a_gate_run_is_persisted_and_read_back_as_a_gate_run`, `a_queued_gate_run_is_restored_as_a_gate_run_in_order`]
- [x] A parameter change initiates no operation (verifies spec: OPS#changes-to-an-app) [`a_param_change_initiates_no_operation`]
- [x] A change is accepted while an operation runs, and refused while another change to the app is being applied (verifies spec: OPS#changes-to-an-app) [`a_change_is_accepted_while_an_operation_runs_for_the_app`, `a_change_is_refused_while_another_is_being_applied`]
- [ ] A failed or cancelled gate run files `gate_failed` and no `operation_failed` (verifies spec: OPS#completion)
- [ ] A replayed gate run gets `last` reconstructed from the generation of the last success, though later generations were committed (verifies spec: OPS#initiating-events)

## Parameter changes

- [x] A change sets and unsets in one generation, with one `param_change` history entry and one `ParamsChanged` event (verifies spec: PRM#parameter-changes) [`a_change_of_several_parameters_is_one_generation_and_one_event`, `set_param_bumps_generation_and_stores_value`, `unset_param_removes_value_and_bumps_generation`]
- [x] A member that changes nothing is left out, and a change with no effective member is a no-op returning the current generation (verifies spec: PRM#parameter-changes) [`members_that_change_nothing_are_left_out_of_the_change`, `set_same_value_is_a_noop`, `unset_never_set_param_is_a_noop`]
- [x] A malformed change is refused with `requirements_invalid` and stores nothing (verifies spec: PRM#parameter-changes) [`a_change_the_server_refuses_as_malformed_stores_nothing`, `a_change_without_params_is_refused`]
- [x] A change for an unregistered app is `not_found`, and one during another change is `operation_in_progress` (verifies spec: PRM#parameter-changes) [`param_calls_on_unknown_app_are_not_found`, `a_change_is_refused_while_another_is_being_applied`]
- [x] A validator sees the whole change, and a rejection stores none of it (verifies spec: PRM#validation) [`validators_see_the_whole_change_and_a_rejection_stores_none_of_it`]
- [x] A value for a parameter the definition does not declare is stored (verifies spec: PRM#parameter-storage) [`set_undeclared_param_is_stored_as_unknown`]
- [x] A parameter that is secret moves to protected storage (verifies spec: PRM#parameter-storage) [`a_param_made_secret_by_a_param_change_moves_to_secret_storage`]
- [x] A plan treats proposed parameters with no members as none, and refuses a name given twice (verifies spec: PRM#change-planning) [`a_plan_treats_proposed_params_with_no_members_as_none_and_refuses_what_a_change_refuses`]
- [x] Install refusals come in the order the spec lists them (verifies spec: ACI#install) [`install_refusals_are_given_in_the_order_the_spec_lists_them`]
- [x] `col` of a gate is an empty collection (verifies spec: COL#collection) [`col_from_gate_yields_empty`]
- [ ] A change whose new values make the definition fail to evaluate files `script_error` and the request still succeeds (verifies spec: PRM#parameter-changes)
- [ ] A change that moves a gate's key leaves it unsatisfied and the gate runs (verifies spec: PRM#parameter-changes)

## Change planning

- [x] A plan lists the gates the proposal would leave unsatisfied, in ascending order (verifies spec: PRM#change-planning) [`a_plan_lists_the_gates_the_proposal_would_leave_unsatisfied`]
- [x] A plan with no proposal lists the gates unsatisfied now, and lists none for an app that is not installed (verifies spec: PRM#change-planning) [`a_plan_with_no_proposal_lists_the_gates_that_are_unsatisfied_now`, `a_plan_for_an_app_that_is_not_installed_lists_no_gates`]
- [x] A plan does not fold an unreadable gate record into an answer (verifies spec: PRM#change-planning) [`a_plan_does_not_fold_an_unreadable_gate_record_into_an_answer`]
- [x] A plan takes several proposed parameters (verifies spec: PRM#change-planning) [`a_plan_takes_several_proposed_parameters`]
- [ ] A plan runs no gate, gate check or action closure (verifies spec: PRM#change-planning)

## Operator control of gates

- [x] A gate run is refused with `not_found`, `not_installed` and `script_error` where the spec says (verifies spec: CTL#gate-runs) [`a_gate_run_is_refused_for_what_is_not_there_or_not_installed`]
- [x] A gate run is refused while a target is not ready, naming each (verifies spec: CTL#gate-runs) [`a_gate_run_is_refused_while_a_target_is_not_ready_naming_each`]
- [x] A gate run is refused while an operation runs or is queued (verifies spec: CTL#gate-runs) [`a_gate_run_is_refused_while_an_operation_runs_or_is_queued`]
- [x] A satisfied gate can be run, and running a paused gate ends its pause (verifies spec: CTL#gate-runs) [`a_gate_run_is_accepted_for_a_satisfied_gate`, `running_a_paused_gate_ends_its_pause`]
- [ ] An operator's gate run is initiated without waiting for its retry to come due, with the trigger `operator` (verifies spec: CTL#gate-runs)
- [x] A restart is refused while a gate holds the Deployment (verifies spec: CTL#deployment-restart) [`a_restart_is_refused_while_a_gate_holds_the_deployment`]
- [x] A resource a gate holds can be stopped (verifies spec: CTL#resource-stop) [`a_resource_a_gate_holds_can_be_stopped`]
- [ ] A resource stopped while a gate holds it stays stopped once the hold ends (verifies spec: CTL#resource-stop)
- [x] `apps show` reports each gate in declaration order with its state and targets, and what holds and awaits each workload (verifies spec: APPS#gates) [`show_reports_each_gate_in_declaration_order_with_its_state_and_targets`, `a_workload_reports_the_targets_it_waits_on_and_the_gates_that_hold_it`]
- [x] Install takes only the app, and refuses a registered app that is already installed (verifies spec: ACI#install) [`install_takes_only_the_app_and_moves_it_straight_to_installed`, `install_refusals`]
- [x] An installed app is `installing` until every gate has been satisfied at once (verifies spec: APPS#status-derivation) [`an_installed_app_is_installing_until_every_gate_has_been_satisfied_at_once`]
- [x] The CLI takes `apps gate run <app> <gate>`, and `apps params set` takes `name=value` pairs and `--unset` (verifies spec: CLI#action-and-shell-parameters) [`gate_run_takes_an_app_and_a_gate`, `params_set_takes_pairs_and_unsets`]

## On a real host

- [ ] Installing an app with a migration gate runs the gate once, holds the exclusive selection stopped while it runs, and leaves the app `running` once it succeeds
- [ ] Changing a keyed parameter through `apps params set` runs the gate with the exclusive selection stopped, and a failing migration keeps the selection stopped and holds `gate_failed` until it is fixed
- [ ] Restoring a database from before a migration makes the gate's check fail, the gate runs again, and `last` is the release the restored data belonged to
- [ ] `apps gate run` on a satisfied gate repeats its body, and cancelling a run mid-body pauses the gate until it is run again
- [ ] Restarting the daemon during a gate run replays the run, and during back-off keeps the delay
