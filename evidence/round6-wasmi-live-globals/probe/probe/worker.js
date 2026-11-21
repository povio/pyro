importScripts('/resources/testharness.js','modules.js','checks.js');
contract((name,f)=>test(f,name),assert_true,assert_equals);asyncContract((name,f)=>promise_test(f,name),assert_equals);done();
