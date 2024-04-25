let inputs = act.inputs();
console.log(`inputs.a=${inputs.a}`);

// set input to inputs.a + 100
// it will show 110 with workflow outputs
act.set('input', inputs.a + 100);

// my_data will export as the step's data
act.set_output('my_data', 'abc');

console.log(`state=${act.state()}`);
