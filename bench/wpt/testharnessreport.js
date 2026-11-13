/*
 * Prepare a report object containing all tests results.
 * https://wpt-docs.readthedocs.io/en/latest/_writing-tests/testharness-api.html#callback-api
 *
 * The testharnessreport.js of lightpanda-io/wpt's fork branch. bench/wpt/serve.mjs serves it at
 * /resources/testharnessreport.js in place of the checkout's own copy; bench/wpt/run.mjs reads window.report.
 */
// No result table in the page and no per-assert record, as upstream wptrunner's
// testharnessreport.js sets for automated runs: with output on, testharness.js
// formats every assert's arguments and takes a stack trace for it, which made
// pages with millions of asserts (encoding/encodeInto-tail-unmodified.any.html:
// about 6 M) run 20 s and cross the 30 s timeout on slow runners.
setup({ output: false });

var report = {
  complete: false,
  // Number of subtests with a result. wptrunner polls it to detect progress.
  completed: 0,
  status: "",
  cases: {},

  get log() {
    const keys = Object.keys(report.cases);
    if (keys.length === 0) {
      return "no test suite completion|Fail|The test never reaches the completion callback.";
    }
    var log = "";
    for (const k of keys) {
      log += report.cases[k] + "\n";
    }
    return log;
  },
  name: function(test) {
    const name = test.name;
    return name ? name.replace(/\n/g, '') : name;
  },
  format: function(test) {
    var log = report.name(test)+"|"+test.format_status();
    if (test.message != null) {
      log +=  "|"+test.message.replaceAll("\n"," ");
    }
    return log;
  }
}

add_test_state_callback(function (test) {
  report.cases[report.name(test)] = report.format(test);
});

add_result_callback(function (test) {
  report.completed++;
  report.cases[report.name(test)] = report.format(test);
});

add_completion_callback(function (tests, status) {
  report.complete = true;
});
