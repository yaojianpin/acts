console.info('running step2 package');

let input = act.get('input');

// get the input value, it should be 110
// the value is calculated by step1
console.log(`get input=${input}`);

// add 10 to the input
// in the end, the input should be 120
act.set('input', input + 10);
