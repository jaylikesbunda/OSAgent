window.OSA = window.OSA || {};

OSA.handleQuestionEvent = function(event) {
    OSA.setPendingQuestionId(event.question_id || '');
    OSA.setPendingQuestions(event.questions || []);
    OSA.setCurrentQuestionIndex(0);
    OSA.setSelectedAnswers([]);
    if (OSA.getPendingQuestions().length === 0) return;
    OSA.renderQuestionModal();
    document.getElementById('question-modal').classList.remove('hidden');
};

OSA.renderQuestionModal = function() {
    const body = document.getElementById('question-body');
    const title = document.getElementById('question-title');
    const pendingQuestions = OSA.getPendingQuestions();
    const currentQuestionIndex = OSA.getCurrentQuestionIndex();
    const selectedAnswers = OSA.getSelectedAnswers();
    
    if (currentQuestionIndex >= pendingQuestions.length) {
        OSA.submitQuestion();
        return;
    }

    const q = pendingQuestions[currentQuestionIndex];
    title.textContent = q.header || 'Question';
    
    let html = `<div class="question-item">
        <div class="question-text">${OSA.escapeHtml(q.question || '')}</div>
        <div class="question-options">`;
    
    const options = q.options || [];
    options.forEach((opt, idx) => {
        const selected = selectedAnswers[currentQuestionIndex]?.includes(opt.label);
        html += `
            <div class="question-option ${selected ? 'selected' : ''}" 
                 onclick="OSA.selectQuestionOption(${currentQuestionIndex}, ${idx}, ${q.multiple || false})">
                <div class="question-option-label">
                    ${OSA.escapeHtml(opt.label || '')}
                    ${opt.label?.toLowerCase().includes('recommended') ? '<span class="recommended">(Recommended)</span>' : ''}
                </div>
                ${opt.description ? `<div class="question-option-desc">${OSA.escapeHtml(opt.description)}</div>` : ''}
            </div>
        `;
    });
    
    html += `
        <div class="question-custom-input">
            <input type="text" id="question-custom-input" placeholder="Type your own answer" 
                   oninput="OSA.updateCustomAnswer(${currentQuestionIndex})"
                   value="${selectedAnswers[currentQuestionIndex]?.find(a => !options.some(o => o.label === a)) || ''}">
        </div>
    `;
    
    html += '</div></div>';
    body.innerHTML = html;
};

OSA.selectQuestionOption = function(qIdx, optIdx, multiple) {
    const pendingQuestions = OSA.getPendingQuestions();
    let selectedAnswers = OSA.getSelectedAnswers();
    const q = pendingQuestions[qIdx];
    if (!q) return;
    
    const opt = q.options[optIdx];
    if (!opt) return;

    if (!selectedAnswers[qIdx]) selectedAnswers[qIdx] = [];
    
    const idx = selectedAnswers[qIdx].indexOf(opt.label);
    if (idx >= 0) {
        selectedAnswers[qIdx].splice(idx, 1);
    } else {
        if (multiple) {
            selectedAnswers[qIdx].push(opt.label);
        } else {
            selectedAnswers[qIdx] = [opt.label];
        }
    }
    
    OSA.setSelectedAnswers(selectedAnswers);
    OSA.renderQuestionModal();
};

OSA.updateCustomAnswer = function(qIdx) {
    const input = document.getElementById('question-custom-input');
    if (!input) return;
    
    const pendingQuestions = OSA.getPendingQuestions();
    let selectedAnswers = OSA.getSelectedAnswers();
    const q = pendingQuestions[qIdx];
    if (!q) return;
    
    const customValue = input.value.trim();
    
    if (!selectedAnswers[qIdx]) selectedAnswers[qIdx] = [];
    
    selectedAnswers[qIdx] = selectedAnswers[qIdx].filter(a => 
        q.options?.some(o => o.label === a)
    );
    
    if (customValue) {
        if (q.multiple) {
            selectedAnswers[qIdx].push(customValue);
        } else {
            selectedAnswers[qIdx] = [customValue];
        }
    }
    
    OSA.setSelectedAnswers(selectedAnswers);
};

OSA.submitQuestion = function() {
    const pendingQuestions = OSA.getPendingQuestions();
    const selectedAnswers = OSA.getSelectedAnswers();
    const questionId = OSA.getPendingQuestionId();

    const answers = pendingQuestions.map((q, idx) => selectedAnswers[idx] || []);

    document.getElementById('question-modal').classList.add('hidden');

    if (questionId) {
        fetch('/api/questions/answer', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                question_id: questionId,
                answers: answers,
            }),
        }).catch(err => console.error('Failed to submit question answer:', err));
    }

    OSA.setPendingQuestions([]);
    OSA.setPendingQuestionId('');
    OSA.setCurrentQuestionIndex(0);
    OSA.setSelectedAnswers([]);
};

window.selectQuestionOption = OSA.selectQuestionOption;
window.updateCustomAnswer = OSA.updateCustomAnswer;
window.submitQuestion = OSA.submitQuestion;
