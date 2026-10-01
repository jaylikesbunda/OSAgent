const assert = require('node:assert/strict');
const test = require('node:test');
global.window = global;
global.OSA = {};
require('../js/goals.js');

test('goal commands separate objectives, controls, and bounded budgets', () => {
    assert.equal(OSA.parseGoalCommand('/goals fix it'), null);
    assert.deepEqual(OSA.parseGoalCommand('/goal'), { action: 'status' });
    assert.deepEqual(OSA.parseGoalCommand('/GOAL --rounds 8 Fix the build'), { action: 'create', objective: 'Fix the build', max_rounds: 8 });
    assert.deepEqual(OSA.parseGoalCommand('/goal resume --rounds 2'), { action: 'resume', max_rounds: 2 });
    for (const command of ['/goal --rounds 0 fix', '/goal --rounds 101 fix', '/goal --rounds 3', '/goal pause something', '/goal status --rounds 5']) {
        assert.throws(() => OSA.parseGoalCommand(command));
    }
});

test('goal control uses the captured session and does not overwrite another chat after a switch', async () => {
    let session = { id: 'session/a' };
    const notices = [];
    OSA.getCurrentSession = () => session;
    OSA.showToast = text => notices.push(text);
    OSA.fetchWithAuth = async (url, options) => {
        assert.equal(url, '/api/sessions/session%2Fa/goal');
        assert.deepEqual(JSON.parse(options.body), { action: 'pause' });
        session = { id: 'other' };
        return { ok: true, json: async () => ({ goal: { phase: 'paused', objective: 'Test', rounds_started: 1, max_rounds: 5 } }) };
    };
    assert.equal(await OSA.handleGoalCommand('/goal pause'), true);
    assert.equal(notices.length, 0);
});

test('failed goal commands report failure so the composer can preserve the draft', async () => {
    OSA.getCurrentSession = () => ({ id: 's' });
    let notice;
    OSA.showToast = text => { notice = text; };
    OSA.fetchWithAuth = async () => ({ ok: false, json: async () => ({ error: 'Goal already exists' }) });
    assert.equal(await OSA.handleGoalCommand('/goal Fix the build'), false);
    assert.equal(notice, 'Goal already exists');
});
