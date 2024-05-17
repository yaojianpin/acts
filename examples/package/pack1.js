console.info('running step1 package');

let inputs = act.inputs();
console.log(`inputs.a=${inputs.a}`);

// set input to inputs.a + 100
// it will show 110 with workflow outputs
act.set('input', inputs.a + 100);

// update hte step data to 'abc'
// the data will send by msg1
act.set('my_data', 'abc');

console.log(`state=${act.state()}`);
