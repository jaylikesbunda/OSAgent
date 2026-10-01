const assert = require('node:assert/strict');
const test = require('node:test');
global.window = global; global.OSA = {};
require('../js/diff-core.js'); require('../js/diff.js');
test('bounded diffs reconstruct both versions even for huge replacements', () => {
    const a = 'prefix\n' + Array.from({length:1200},(_,i)=>'old '+i).join('\n') + '\nsuffix';
    const b = 'prefix\n' + Array.from({length:1200},(_,i)=>'new '+i).join('\n') + '\nsuffix';
    const {lines} = OSA.computeLineDiff(a,b);
    assert.equal(lines.filter(line=>line.type!=='add').map(line=>line.text).join('\n'),a);
    assert.equal(lines.filter(line=>line.type!=='del').map(line=>line.text).join('\n'),b);
});
test('concurrent diff worker replies are routed to their own file', async () => {
    global.Worker = class {
        listeners = {}; requests = [];
        addEventListener(type,handler) {this.listeners[type]=handler;}
        postMessage(request) {this.requests.push(request);}
    };
    OSA._diffWorker=null;
    const first = OSA.computeLineDiffAsync('a'.repeat(50001),'first');
    const second = OSA.computeLineDiffAsync('b'.repeat(50001),'second');
    const worker=OSA._diffWorker;
    const [a,b]=worker.requests;
    worker.listeners.message({data:{id:b.id,lines:['second']}});
    worker.listeners.message({data:{id:a.id,lines:['first']}});
    assert.deepEqual((await first).lines,['first']);
    assert.deepEqual((await second).lines,['second']);
    delete global.Worker;
});
