const assert = require('node:assert/strict');
const test = require('node:test');
let session;
test.before(async () => {
    const { Window } = await import('happy-dom');
    global.window = new Window();
    global.document = window.document;
    global.OSA = window.OSA = {};
    require('../js/goals.js');
    OSA.getCurrentSession = () => session;
});
test.beforeEach(() => {
    document.body.innerHTML = '<section id="goal-panel"></section><button id="goal-trigger"></button><form id="goal-editor" class="hidden"><textarea id="goal-objective"></textarea></form>';
    session = { id: 'one' };
    OSA._goalActionPending = false;
    OSA._goalSnapshot = { armed: true, goal: { id: 'g', phase: 'active', objective: '<img src=x onerror=alert(1)> Fix the build', rounds_started: 2, max_rounds: 5 } };
});
test.afterEach(() => clearTimeout(OSA._goalPoll));
test('goal panel renders text safely and preserves options and budget edits during progress', () => {
    OSA.renderGoalPanel();
    assert.equal(document.querySelector('#goal-panel img'), null);
    assert.match(document.querySelector('.goal-objective').textContent, /<img/);
    const options = document.querySelector('details');
    options.open = true;
    document.getElementById('goal-resume-rounds').value = '8';
    OSA._goalSnapshot.goal.rounds_started = 3;
    OSA.renderGoalPanel();
    assert.equal(document.querySelector('details').open, true);
    assert.equal(document.getElementById('goal-resume-rounds').value, '8');
    assert.match(document.querySelector('.goal-meta').textContent, /3 \/ 5 rounds/);
});
test('a goal with no round limit is labelled and offers an unlimited resume', () => {
    OSA._goalSnapshot.goal.max_rounds = 0;
    OSA._goalSnapshot.goal.rounds_started = 4;
    OSA.renderGoalPanel();
    assert.match(document.querySelector('.goal-meta').textContent, /4 rounds, no limit/);
    const unlimited = document.getElementById('goal-resume-unlimited');
    assert.equal(unlimited.checked, true);
    assert.equal(document.getElementById('goal-resume-rounds').disabled, true);
});
test('a restart-disarmed goal offers Resume rather than claiming it is active', () => {
    OSA._goalSnapshot.armed = false;
    OSA.renderGoalPanel();
    assert.equal(document.querySelector('.goal-eyebrow').textContent, 'Goal · Paused');
    assert.equal(document.querySelector('.goal-row button').textContent, 'Resume');
    assert.match(document.querySelector('.goal-note').textContent, /restart/);
});
test('clear requires a deliberate second click', () => {
    OSA.renderGoalPanel();
    const actions = [];
    OSA.runGoalPanelAction = action => actions.push(action);
    const clear = document.querySelector('.goal-options-body button');
    clear.click();
    assert.deepEqual(actions, []);
    assert.equal(clear.textContent, 'Confirm clear');
    clear.click();
    assert.deepEqual(actions, ['clear']);
});
test('a late goal response cannot overwrite the panel after switching chats', async () => {
    OSA._goalViewGeneration = 1;
    OSA.fetchWithAuth = async () => {
        session = { id: 'two' };
        OSA._goalViewGeneration++;
        return { ok: true, json: async () => ({ goal: { objective: 'Old goal' } }) };
    };
    const snapshot = OSA._goalSnapshot;
    await OSA.refreshGoalPanel();
    assert.equal(OSA._goalSnapshot, snapshot);
});
