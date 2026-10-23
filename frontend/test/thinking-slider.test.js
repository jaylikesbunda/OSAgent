const assert = require('node:assert/strict');
const test = require('node:test');

let Window;

test.before(async () => {
    ({ Window } = await import('happy-dom'));
    const window = new Window({ url: 'http://localhost/' });
    global.window = window;
    global.document = window.document;
    global.navigator = window.navigator;
    global.localStorage = window.localStorage;
    global.Node = window.Node;
    global.HTMLElement = window.HTMLElement;
    global.Event = window.Event;
    global.requestAnimationFrame = () => 1;
    global.cancelAnimationFrame = () => {};
    global.OSA = window.OSA = {};

    require('../js/state.js');
    require('../js/utils.js');
    require('../js/settings.js');
});

const MARKUP = `
    <select id="header-thinking-level" class="hidden">
        <option value="auto">Auto</option>
        <option value="off">Off</option>
        <option value="low">Low</option>
        <option value="medium">Medium</option>
        <option value="high">High</option>
    </select>
    <span id="header-thinking-label">Auto</span>
    <div id="header-thinking-menu" class="model-picker-popover hidden">
        <div class="thinking-slider-panel">
            <div class="thinking-slider-head">
                <span id="header-thinking-level-name">Auto</span>
                <span id="header-thinking-model-name"></span>
            </div>
            <div class="thinking-slider" id="header-thinking-slider" role="slider" tabindex="0">
                <div class="thinking-slider-track">
                    <div class="thinking-slider-fill" id="header-thinking-fill"></div>
                    <div class="thinking-slider-stops" id="header-thinking-stops"></div>
                    <div class="thinking-slider-thumb" id="header-thinking-thumb"></div>
                </div>
            </div>
        </div>
    </div>
`;

function setup(selected) {
    document.body.innerHTML = MARKUP;
    const select = document.getElementById('header-thinking-level');
    select.value = selected || 'auto';
    OSA.currentModelId = 'claude-sonnet-4-5';
    OSA.currentModelProviderId = 'anthropic';
    OSA.providerCatalog = {
        providers: [{ id: 'anthropic', name: 'Anthropic', connected: true }],
        all_models: [],
        model_menu: {},
    };
    OSA.renderThinkingMenu();
    return select;
}

function trackGeometry() {
    const track = document.querySelector('#header-thinking-slider .thinking-slider-track');
    // happy-dom performs no layout; give the slider a geometry to work with.
    track.getBoundingClientRect = () => ({
        left: 0,
        width: 100,
        top: 0,
        height: 20,
        right: 100,
        bottom: 20,
    });
    return track;
}

test('the composer thinking control is a slider with one stop per level', () => {
    setup('auto');

    const stops = document.querySelectorAll('#header-thinking-stops .thinking-slider-stop');
    assert.equal(stops.length, 5);
    assert.equal(stops[0].style.left, '0%');
    assert.equal(stops[4].style.left, '100%');
    assert.equal(document.getElementById('header-thinking-level-name').textContent, 'Auto');
    assert.equal(document.getElementById('header-thinking-label').textContent, 'Auto');
    assert.equal(
        document.getElementById('header-thinking-model-name').textContent,
        'Anthropic · claude-sonnet-4-5'
    );
    assert.equal(document.getElementById('header-thinking-fill').style.width, 'calc(0% + var(--thinking-pill-inset))');
});

test('the slider reflects the selected level', () => {
    setup('medium');

    assert.equal(document.getElementById('header-thinking-level-name').textContent, 'Medium');
    // "medium" is the fourth of five stops (auto, off, low, medium, high). The
    // fill starts one knob radius before the travel range (CSS owns the inset).
    assert.equal(document.getElementById('header-thinking-fill').style.width, 'calc(75% + var(--thinking-pill-inset))');
    assert.equal(document.getElementById('header-thinking-thumb').style.left, '75%');

    const slider = document.getElementById('header-thinking-slider');
    assert.equal(slider.getAttribute('aria-valuemin'), '0');
    assert.equal(slider.getAttribute('aria-valuemax'), '4');
    assert.equal(slider.getAttribute('aria-valuenow'), '3');
    assert.equal(slider.getAttribute('aria-valuetext'), 'Medium');

    // The stop under the knob is highlighted.
    const current = document.querySelectorAll('#header-thinking-stops .thinking-slider-stop.is-current');
    assert.equal(current.length, 1);
    assert.equal(current[0].style.left, '75%');
});

test('a model without reasoning levels renders an inert single-stop slider', () => {
    document.body.innerHTML = MARKUP;
    const select = document.getElementById('header-thinking-level');
    select.innerHTML = '<option value="auto">Auto</option>';
    select.value = 'auto';
    OSA.providerCatalog = { providers: [], all_models: [], model_menu: {} };
    OSA.renderThinkingMenu();

    const slider = document.getElementById('header-thinking-slider');
    assert.ok(slider.classList.contains('is-single'));
    assert.equal(slider.getAttribute('tabindex'), '-1');
});

test('keyboard changes commit through the hidden select', () => {
    const select = setup('medium');
    let changed = null;
    select.addEventListener('change', () => { changed = select.value; });

    const slider = document.getElementById('header-thinking-slider');
    slider.dispatchEvent(new window.KeyboardEvent('keydown', {
        key: 'ArrowRight',
        bubbles: true,
        cancelable: true,
    }));

    assert.equal(select.value, 'high');
    assert.equal(changed, 'high');
    assert.equal(document.getElementById('header-thinking-level-name').textContent, 'High');
    // The level label re-triggers its bump animation on every change.
    assert.ok(document.getElementById('header-thinking-level-name').classList.contains('is-bump'));
});

test('dragging marks the slider as held and releases it afterwards', () => {
    setup('auto');
    trackGeometry();

    const slider = document.getElementById('header-thinking-slider');
    slider.dispatchEvent(new window.MouseEvent('pointerdown', {
        clientX: 25,
        button: 0,
        bubbles: true,
        cancelable: true,
    }));
    assert.ok(slider.classList.contains('is-dragging'));

    // Moving across the track previews the knob without a save.
    slider.dispatchEvent(new window.MouseEvent('pointermove', {
        clientX: 100,
        button: 0,
        bubbles: true,
        cancelable: true,
    }));
    assert.equal(document.getElementById('header-thinking-level-name').textContent, 'High');

    slider.dispatchEvent(new window.MouseEvent('pointerup', {
        clientX: 100,
        button: 0,
        bubbles: true,
        cancelable: true,
    }));
    assert.equal(slider.classList.contains('is-dragging'), false);
});

test('pointer position picks the nearest stop and commits on release', () => {
    const select = setup('auto');
    trackGeometry();

    const slider = document.getElementById('header-thinking-slider');
    // The far end is the last stop, "High".
    slider.dispatchEvent(new window.MouseEvent('pointerdown', {
        clientX: 100,
        button: 0,
        bubbles: true,
        cancelable: true,
    }));
    assert.equal(document.getElementById('header-thinking-level-name').textContent, 'High');
    assert.equal(select.value, 'auto', 'dragging previews without persisting');

    slider.dispatchEvent(new window.MouseEvent('pointerup', {
        clientX: 100,
        button: 0,
        bubbles: true,
        cancelable: true,
    }));
    assert.equal(select.value, 'high');
});

test('stopping on the same level does not refire a save', () => {
    const select = setup('medium');
    trackGeometry();

    let changes = 0;
    select.addEventListener('change', () => { changes += 1; });

    const slider = document.getElementById('header-thinking-slider');
    // 75% is "Medium", the level already selected.
    slider.dispatchEvent(new window.MouseEvent('pointerdown', {
        clientX: 75,
        button: 0,
        bubbles: true,
        cancelable: true,
    }));
    slider.dispatchEvent(new window.MouseEvent('pointerup', {
        clientX: 75,
        button: 0,
        bubbles: true,
        cancelable: true,
    }));

    assert.equal(changes, 0);
    assert.equal(select.value, 'medium');
});

test('dragging past the track clamps to the ends', () => {
    const select = setup('medium');
    trackGeometry();

    const slider = document.getElementById('header-thinking-slider');
    slider.dispatchEvent(new window.MouseEvent('pointerdown', {
        clientX: 500,
        button: 0,
        bubbles: true,
        cancelable: true,
    }));
    slider.dispatchEvent(new window.MouseEvent('pointerup', {
        clientX: 500,
        button: 0,
        bubbles: true,
        cancelable: true,
    }));
    assert.equal(select.value, 'high');

    slider.dispatchEvent(new window.MouseEvent('pointerdown', {
        clientX: -40,
        button: 0,
        bubbles: true,
        cancelable: true,
    }));
    slider.dispatchEvent(new window.MouseEvent('pointerup', {
        clientX: -40,
        button: 0,
        bubbles: true,
        cancelable: true,
    }));
    assert.equal(select.value, 'auto');
});
