// ==UserScript==
// @name         Neusoft Course Grabber
// @namespace    http://tampermonkey.net/
// @version      2.0
// @description  东软自动抢课脚本 - 多关键词优先级、极速模式、定时抢课、自动确认、重试机制
// @author       LuBanQAQ & qlAD
// @match        *://xk.neusoft.edu.cn/xsxk/elective/*
// @icon         data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'><text x='50%' y='50%' dominant-baseline='central' text-anchor='middle' font-size='52'>🚀</text></svg>
// @icon64       data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'><text x='50%' y='50%' dominant-baseline='central' text-anchor='middle' font-size='52'>🚀</text></svg>
// @grant        none
// @run-at       document-end
// ==/UserScript==

(function() {
    'use strict';

    // 防止 SPA 页面切换导致重复注入
    if (document.getElementById('nscg-panel')) return;

    // --- GUI CSS ---
    const css = `
        #nscg-panel {
            position: fixed;
            top: 50px;
            right: 50px;
            width: 380px;
            max-height: 86vh;
            background: rgba(255, 255, 255, 0.96);
            border: 1px solid #dcdfe6;
            box-shadow: 0 4px 16px rgba(0, 0, 0, 0.15);
            border-radius: 12px;
            z-index: 99999;
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            font-size: 14px;
            color: #303133;
            display: flex;
            flex-direction: column;
            transition: opacity 0.3s;
            overflow: hidden;
        }
        #nscg-header {
            padding: 12px 14px;
            background: linear-gradient(135deg, #409EFF, #3a8ee6);
            color: white;
            font-weight: 600;
            border-radius: 12px 12px 0 0;
            cursor: move;
            display: flex;
            justify-content: space-between;
            align-items: center;
            user-select: none;
        }
        .nscg-title-wrap {
            display: flex;
            flex-direction: column;
            gap: 2px;
        }
        .nscg-title {
            font-size: 15px;
            line-height: 1.1;
        }
        .nscg-subtitle {
            font-size: 11px;
            opacity: 0.9;
            font-weight: 400;
        }
        .nscg-circle-btn {
            width: 24px;
            height: 24px;
            border-radius: 50%;
            border: 1px solid rgba(255, 255, 255, 0.45);
            background: rgba(255, 255, 255, 0.2);
            color: #fff;
            font-size: 14px;
            line-height: 1;
            display: inline-flex;
            align-items: center;
            justify-content: center;
            cursor: pointer;
            padding: 0;
            transition: background 0.2s, transform 0.2s;
        }
        .nscg-circle-btn:hover {
            background: rgba(255, 255, 255, 0.35);
            transform: scale(1.06);
        }
        .nscg-circle-btn:active {
            transform: scale(0.96);
        }
        #nscg-content {
            padding: 12px;
            background: #fff;
            border-radius: 0 0 12px 12px;
            overflow-y: auto;
        }
        .nscg-section {
            margin-bottom: 10px;
            padding: 10px;
            border: 1px solid #ebeef5;
            border-radius: 8px;
            background: #fcfdff;
        }
        .nscg-section:last-of-type {
            margin-bottom: 0;
        }
        .nscg-section-title {
            font-size: 12px;
            color: #909399;
            font-weight: 600;
            margin-bottom: 8px;
            letter-spacing: 0.2px;
        }
        .nscg-row {
            margin-bottom: 8px;
            display: flex;
            align-items: center;
            gap: 8px;
        }
        .nscg-row:last-child {
            margin-bottom: 0;
        }
        .nscg-label {
            width: 96px;
            font-size: 13px;
            color: #606266;
            flex-shrink: 0;
        }
        .nscg-input {
            flex: 1;
            padding: 6px 10px;
            border: 1px solid #dcdfe6;
            border-radius: 4px;
            outline: none;
            font-size: 13px;
            transition: border-color 0.2s;
            min-width: 0;
        }
        .nscg-input:focus {
            border-color: #409EFF;
        }
        .nscg-grid {
            display: grid;
            grid-template-columns: repeat(2, minmax(0, 1fr));
            gap: 8px;
        }
        .nscg-grid .nscg-row {
            margin-bottom: 0;
            min-width: 0;
            overflow: hidden;
        }
        .nscg-grid .nscg-label {
            width: 58px;
            flex: 0 0 58px;
            font-size: 12px;
        }
        .nscg-grid .nscg-input {
            min-width: 0;
            width: 0;
            flex: 1 1 auto;
        }
        @media (max-width: 420px) {
            #nscg-panel {
                width: min(96vw, 380px);
                right: 2vw;
            }
            .nscg-grid {
                grid-template-columns: 1fr;
            }
            .nscg-grid .nscg-label {
                width: 66px;
                flex-basis: 66px;
            }
        }
        .nscg-btn {
            width: 100%;
            padding: 10px;
            border: none;
            border-radius: 4px;
            background: #409EFF;
            color: white;
            cursor: pointer;
            font-weight: 600;
            font-size: 14px;
            transition: all 0.2s;
            margin-top: 0;
        }
        .nscg-btn:hover {
            background: #66b1ff;
            transform: translateY(-1px);
        }
        .nscg-btn:active {
            transform: translateY(0);
        }
        .nscg-btn.stop {
            background: #F56C6C;
        }
        .nscg-btn.stop:hover {
            background: #f78989;
        }
        .nscg-actions {
            margin-top: 10px;
            display: grid;
            grid-template-columns: 1fr 90px;
            gap: 8px;
        }
        #nscg-log {
            margin-top: 10px;
            height: 160px;
            overflow-y: auto;
            border: 1px solid #ebeef5;
            background: #fafafa;
            padding: 8px;
            font-size: 12px;
            color: #909399;
            border-radius: 4px;
            font-family: Consolas, monospace;
        }
        .nscg-log-item {
            margin-bottom: 4px;
            line-height: 1.4;
            border-bottom: 1px dashed #eee;
        }
        .nscg-status {
            text-align: center;
            margin-bottom: 10px;
            padding: 8px;
            background: #f0f9eb;
            color: #67c23a;
            border-radius: 4px;
            font-size: 13px;
            font-weight: 500;
        }
        .nscg-status.waiting {
            background: #fdf6ec;
            color: #e6a23c;
        }
        .nscg-hint {
            margin-top: 6px;
            font-size: 11px;
            color: #909399;
            line-height: 1.3;
        }
        .nscg-fast-row {
            gap: 8px;
            flex-wrap: nowrap;
        }
        .nscg-fast-toggle {
            cursor: pointer;
            display: flex;
            align-items: center;
            font-size: 12px;
            color: #606266;
            flex: 1;
            min-width: 0;
        }
        .nscg-fast-size-label {
            width: 66px;
            flex: 0 0 66px;
            font-size: 12px;
            text-align: right;
        }
        .nscg-fast-size-input {
            width: 48px;
            flex: 0 0 48px;
        }
        @media (max-width: 420px) {
            .nscg-fast-row {
                flex-wrap: wrap;
            }
            .nscg-fast-size-label {
                text-align: left;
            }
            .nscg-fast-size-input {
                flex: 1;
                width: auto;
            }
        }
    `;

    // Inject styles
    const style = document.createElement('style');
    style.textContent = css;
    document.head.appendChild(style);

    // --- GUI HTML ---
    const panel = document.createElement('div');
    panel.id = 'nscg-panel';
    panel.innerHTML = `
        <div id="nscg-header">
            <div class="nscg-title-wrap">
                <span class="nscg-title">东软抢课助手 Pro</span>
                <span class="nscg-subtitle">关键词优先 + 极速模式</span>
            </div>
            <button id="nscg-min" class="nscg-circle-btn" title="最小化" type="button">−</button>
        </div>
        <div id="nscg-content">
            <div class="nscg-status waiting" id="nscg-status">等待配置...</div>

            <div class="nscg-section">
                <div class="nscg-section-title">抢课设置</div>
                <div class="nscg-row">
                    <span class="nscg-label">开始时间:</span>
                    <input type="datetime-local" id="nscg-start-time" class="nscg-input">
                </div>
                <div class="nscg-row">
                    <span class="nscg-label">最大重试:</span>
                    <input type="number" id="nscg-retry" class="nscg-input" value="100" min="1">
                </div>
                <div class="nscg-row">
                    <span class="nscg-label">点击冷却(ms):</span>
                    <input type="number" id="nscg-interval" class="nscg-input" value="1500" min="1" step="100" placeholder="点击后的等待时间">
                </div>
            </div>

            <div class="nscg-section">
                <div class="nscg-section-title">关键词（按顺序优先）</div>
                <div class="nscg-grid">
                    <div class="nscg-row">
                        <span class="nscg-label">关键词1:</span>
                        <input type="text" id="nscg-keyword-1" class="nscg-input" placeholder="如: 戏剧">
                    </div>
                    <div class="nscg-row">
                        <span class="nscg-label">关键词2:</span>
                        <input type="text" id="nscg-keyword-2" class="nscg-input" placeholder="可留空">
                    </div>
                    <div class="nscg-row">
                        <span class="nscg-label">关键词3:</span>
                        <input type="text" id="nscg-keyword-3" class="nscg-input" placeholder="可留空">
                    </div>
                    <div class="nscg-row">
                        <span class="nscg-label">关键词4:</span>
                        <input type="text" id="nscg-keyword-4" class="nscg-input" placeholder="可留空">
                    </div>
                </div>
                <div class="nscg-hint">留空表示忽略；命中多个关键词时优先选择序号更小的关键词。</div>
            </div>

            <div class="nscg-section">
                <div class="nscg-section-title">高级选项</div>
                <div class="nscg-row nscg-fast-row">
                    <label class="nscg-fast-toggle">
                        <input type="checkbox" id="nscg-fast-mode" checked>
                        <span style="margin-left:4px;">⚡ 极速模式（增大每页条数）</span>
                    </label>
                    <span class="nscg-label nscg-fast-size-label">每页条数:</span>
                    <input type="number" id="nscg-page-size" class="nscg-input nscg-fast-size-input" value="200" min="10" step="10">
                </div>
                <div class="nscg-row">
                    <label style="cursor:pointer;display:flex;align-items:center;font-size:12px;color:#F56C6C;flex:1;">
                        <input type="checkbox" id="nscg-trap">
                        <span style="margin-left:4px;">🛡️ 拦截跳转(调试)</span>
                    </label>
                </div>
            </div>

            <div class="nscg-actions">
                <button id="nscg-toggle-btn" class="nscg-btn">🔥 开始抢课</button>
                <button id="nscg-clear-btn" class="nscg-btn" style="background:#909399;font-size:12px;padding:6px;">清空日志</button>
            </div>

            <div id="nscg-log">
                <div class="nscg-log-item">日志控制台就绪...</div>
            </div>
        </div>
    `;
    document.body.appendChild(panel);

    // --- Logic Variables ---
    let isRunning = false;
    let timer = null;
    let retryCount = 0;
    let runKeywords = [];
    let currentKeywordIndex = -1;
    let lastActionTime = 0; // 防止操作过快
    let lastFastModeApplyTime = 0;
    const fastModeApplyCooldown = 2000;
    let suppressStrictUntil = 0;
    let lastConfirmClickTime = 0;
    const confirmClickCooldown = 500;
    let lastProgressLogTime = 0;
    let lastIdleTickTime = 0;
    
    // Elements
    const statusEl = document.getElementById('nscg-status');
    const toggleBtn = document.getElementById('nscg-toggle-btn');
    const clearBtn = document.getElementById('nscg-clear-btn');
    const logEl = document.getElementById('nscg-log');
    
    const startTimeInput = document.getElementById('nscg-start-time');
    const retryInput = document.getElementById('nscg-retry');
    const fastModeInput = document.getElementById('nscg-fast-mode');
    const pageSizeInput = document.getElementById('nscg-page-size');
    const intervalInput = document.getElementById('nscg-interval');
    const keyword1Input = document.getElementById('nscg-keyword-1');
    const keyword2Input = document.getElementById('nscg-keyword-2');
    const keyword3Input = document.getElementById('nscg-keyword-3');
    const keyword4Input = document.getElementById('nscg-keyword-4');
    const trapInput = document.getElementById('nscg-trap');

    // Init Start Time to Now
    const now = new Date();
    now.setMinutes(now.getMinutes() - now.getTimezoneOffset());
    startTimeInput.value = now.toISOString().slice(0, 16);

    // Anti-Kick Trap (Navigation Interceptor)
    window.addEventListener('beforeunload', (e) => {
        if (trapInput.checked) {
            // Cancel the event
            e.preventDefault(); 
            // Chrome requires returnValue to be set
            e.returnValue = '抢课助手已拦截跳转！请检查控制台(F12)查看是谁触发了跳转。';
            log('⛔ 拦截到页面跳转！可能是被踢出或重定向，请检查Call Stack', 'error');
            return e.returnValue;
        }
    });

    // Config Persistence
    function saveConfig() {
        const config = {
            start: startTimeInput.value,
            keyword1: keyword1Input.value,
            keyword2: keyword2Input.value,
            keyword3: keyword3Input.value,
            keyword4: keyword4Input.value,
            trap: trapInput.checked,
            retry: retryInput.value,
            fastMode: fastModeInput.checked,
            pageSize: pageSizeInput.value,
            interval: intervalInput.value
        };
        localStorage.setItem('nscg_config', JSON.stringify(config));
    }

    function loadConfig() {
        const saved = localStorage.getItem('nscg_config');
        if (saved) {
            try {
                const c = JSON.parse(saved);
                if(c.start) startTimeInput.value = c.start;
                if(c.keyword !== undefined && c.keyword1 === undefined) keyword1Input.value = c.keyword;
                if(c.keyword1 !== undefined) keyword1Input.value = c.keyword1;
                if(c.keyword2 !== undefined) keyword2Input.value = c.keyword2;
                if(c.keyword3 !== undefined) keyword3Input.value = c.keyword3;
                if(c.keyword4 !== undefined) keyword4Input.value = c.keyword4;
                if(c.trap !== undefined) trapInput.checked = c.trap;
                if(c.retry) retryInput.value = c.retry;
                if(c.fastMode !== undefined) fastModeInput.checked = c.fastMode;
                if(c.pageSize) pageSizeInput.value = c.pageSize;
                if(c.interval) intervalInput.value = c.interval;
            } catch(e) {}
        }
    }
    loadConfig();

    // Logger
    function log(msg, type = 'info') {
        const time = new Date().toLocaleTimeString('zh-CN', {hour12: false});
        const div = document.createElement('div');
        div.className = 'nscg-log-item';
        div.textContent = `[${time}] ${msg}`;
        if (type === 'error') div.style.color = '#F56C6C';
        if (type === 'success') div.style.color = '#67C23A';
        logEl.appendChild(div);
        logEl.scrollTop = logEl.scrollHeight;
    }
    
    // Clear Log
    clearBtn.addEventListener('click', () => {
        logEl.innerHTML = '';
        log('日志已清空');
    });

    // Draggable Logic
    const header = document.getElementById('nscg-header');
    let isDragging = false;
    let startX, startY, initialLeft, initialTop;

    header.addEventListener('mousedown', (e) => {
        if (e.target.closest && e.target.closest('#nscg-min')) return;
        isDragging = true;
        startX = e.clientX;
        startY = e.clientY;
        const rect = panel.getBoundingClientRect();
        initialLeft = rect.left;
        initialTop = rect.top;
        header.style.cursor = 'grabbing';
    });

    document.addEventListener('mousemove', (e) => {
        if (!isDragging) return;
        e.preventDefault();
        const dx = e.clientX - startX;
        const dy = e.clientY - startY;
        panel.style.left = `${initialLeft + dx}px`;
        panel.style.top = `${initialTop + dy}px`;
        panel.style.right = 'auto';
    });

    document.addEventListener('mouseup', () => {
        isDragging = false;
        header.style.cursor = 'move';
    });
    
    // Toggle Minimize
    const minBtn = document.getElementById('nscg-min');
    minBtn.addEventListener('click', () => {
        const content = document.getElementById('nscg-content');
        if (content.style.display === 'none') {
            content.style.display = 'block';
            minBtn.textContent = '−';
            minBtn.title = '最小化';
        } else {
            content.style.display = 'none';
            minBtn.textContent = '□';
            minBtn.title = '最大化';
        }
    });

    // --- Core Logic ---

    // Step 1) 基础数据处理：容量判断、文本规范化、数字提取
    function isFull(selected, capacity) {
        const s = parseInt(selected, 10);
        const c = parseInt(capacity, 10);
        if (Number.isNaN(s) || Number.isNaN(c)) return false;
        return s >= c;
    }

    function normalizeText(text) {
        return (text || '').replace(/\s+/g, ' ').trim();
    }

    function normalizeComparableText(text) {
        return normalizeText(text)
            .toLowerCase()
            .replace(/[\s\u3000\-_/|]+/g, '')
            .replace(/[，。！？；：、,.!?;:()（）【】\[\]"'“”‘’]/g, '');
    }

    function findFieldValueByLabel(card, labelKeyword) {
        const items = card.querySelectorAll('.card-item');
        for (const item of items) {
            const labelEl = item.querySelector('.label');
            const valueEl = item.querySelector('.value');
            if (!labelEl || !valueEl) continue;
            const label = normalizeText(labelEl.textContent);
            if (label.includes(labelKeyword)) {
                return normalizeText(valueEl.textContent);
            }
        }
        return '';
    }

    function parseFirstNumber(text) {
        const match = normalizeText(text).match(/\d+/);
        return match ? parseInt(match[0], 10) : NaN;
    }

    // Step 2) 关键词处理：索引越小优先级越高
    function getKeywordList() {
        return [
            keyword1Input.value,
            keyword2Input.value,
            keyword3Input.value,
            keyword4Input.value
        ].map(k => normalizeText(k)).filter(Boolean);
    }

    function getMatchedKeywordIndex(courseName, keywords) {
        if (!keywords || keywords.length === 0) return -1;
        const normalizedCourseName = normalizeComparableText(courseName);
        for (let i = 0; i < keywords.length; i++) {
            const normalizedKeyword = normalizeComparableText(keywords[i]);
            if (normalizedKeyword && normalizedCourseName.includes(normalizedKeyword)) {
                return i;
            }
        }
        return -1;
    }

    function hasNextKeyword() {
        return runKeywords.length > 0 && currentKeywordIndex >= 0 && currentKeywordIndex < runKeywords.length - 1;
    }

    function getActiveKeywords() {
        if (runKeywords.length === 0 || currentKeywordIndex < 0) return [];
        return [runKeywords[currentKeywordIndex]];
    }

    function switchToNextKeyword(reason = '') {
        if (!hasNextKeyword()) return false;
        currentKeywordIndex++;
        retryCount = 0;
        suppressStrictUntil = Date.now() + 1200;
        const current = runKeywords[currentKeywordIndex];
        log(`切换关键词: ${current}${reason ? `（${reason}）` : ''}`);
        return true;
    }

    // Step 3) 极速模式：提高每页条数
    function getGrablessonsVueInstance() {
        if (window.grablessonsVue) return window.grablessonsVue;
        if (window.parent && window.parent !== window && window.parent.grablessonsVue) {
            return window.parent.grablessonsVue;
        }
        return null;
    }

    function tryApplyFastModePageSize() {
        if (!fastModeInput.checked) return { ok: false, refreshed: false };

        const now = Date.now();
        if (now - lastFastModeApplyTime < fastModeApplyCooldown) return { ok: false, refreshed: false };
        lastFastModeApplyTime = now;

        const desiredPageSize = Math.max(10, parseInt(pageSizeInput.value, 10) || 200);
        const vue = getGrablessonsVueInstance();
        if (!vue || !vue.pubParam) return { ok: false, refreshed: false };

        const currentPageSize = parseInt(vue.pubParam.pageSize, 10);
        if (!Number.isNaN(currentPageSize) && currentPageSize >= desiredPageSize) {
            return { ok: true, refreshed: false };
        }

        try {
            vue.pubParam.pageSize = desiredPageSize;
            vue.pubParam.pageNumber = 1;

            if (typeof vue.searchCourse === 'function') {
                vue.searchCourse(false);
            } else if (typeof vue.handleCurrentChange === 'function') {
                vue.handleCurrentChange(1);
            }
            return { ok: true, refreshed: true };
        } catch (e) {
            log(`极速模式切换失败：${e.message || e}`, 'error');
            return { ok: false, refreshed: false };
        }
    }

    // Step 4A) 表格视图扫描
    function findBestLessonFromTable(keywords = getKeywordList()) {
        const rows = document.querySelectorAll('tr.el-table__row');
        let matchCount = 0;
        let totalVisible = 0;
        let bestCandidate = null;

        for (const row of rows) {
            if (row.offsetParent === null) continue;
            totalVisible++;

            const cells = row.querySelectorAll('td');
            const courseName = normalizeText(cells[1]?.textContent || '') || '未知课程';
            const matchedKeywordIndex = keywords.length === 0 ? -1 : getMatchedKeywordIndex(courseName, keywords);
            const matchesKeyword = keywords.length === 0 ? true : matchedKeywordIndex >= 0;
            if (matchesKeyword) matchCount++;
            else continue;

            const rowText = normalizeText(row.textContent);
            const hasConflict = rowText.includes('课程冲突') || !!row.querySelector('.el-tag--danger');
            if (hasConflict) continue;

            const capacity = parseFirstNumber(cells[4]?.textContent || '');
            const selected = parseFirstNumber(cells[5]?.textContent || '');
            if (!Number.isNaN(capacity) && !Number.isNaN(selected) && isFull(selected, capacity)) {
                continue;
            }

            const btns = row.querySelectorAll('button.el-button--primary');
            for (const b of btns) {
                if (normalizeText(b.innerText).includes('选择') && !b.disabled) {
                    const candidate = {
                        btn: b,
                        name: courseName,
                        matchCount,
                        totalVisible,
                        matchedKeywordIndex,
                        matchedKeyword: matchedKeywordIndex >= 0 ? keywords[matchedKeywordIndex] : ''
                    };

                    if (!bestCandidate) {
                        bestCandidate = candidate;
                    } else {
                        const currentScore = candidate.matchedKeywordIndex < 0 ? Number.MAX_SAFE_INTEGER : candidate.matchedKeywordIndex;
                        const bestScore = bestCandidate.matchedKeywordIndex < 0 ? Number.MAX_SAFE_INTEGER : bestCandidate.matchedKeywordIndex;
                        if (currentScore < bestScore) {
                            bestCandidate = candidate;
                        }
                    }
                    break;
                }
            }
        }

        if (bestCandidate) {
            bestCandidate.matchCount = matchCount;
            bestCandidate.totalVisible = totalVisible;
            return bestCandidate;
        }

        return { btn: null, matchCount, totalVisible, matchedKeyword: '', matchedKeywordIndex: -1 };
    }

    // Step 4B) 卡片视图扫描：兼容不同页面结构
    function findBestLessonFromCards(keywords = getKeywordList()) {
        const cards = document.querySelectorAll('.course-list .el-card.jxb-card');
        let matchCount = 0;
        let totalVisible = 0;
        let bestCandidate = null;

        for (let i = 0; i < cards.length; i++) {
            const card = cards[i];

            if (card.offsetParent === null) continue;
            totalVisible++;

            const courseName = findFieldValueByLabel(card, '课程名称') || '未知课程';
            const matchedKeywordIndex = keywords.length === 0 ? -1 : getMatchedKeywordIndex(courseName, keywords);
            const matchesKeyword = keywords.length === 0 ? true : matchedKeywordIndex >= 0;

            if (matchesKeyword) matchCount++;
            else continue;

            const tags = card.querySelectorAll('.el-tag--danger');
            let hasConflict = false;
            for (let t of tags) {
                if (t.innerText.includes('冲突')) { hasConflict = true; break; }
            }
            if (!hasConflict && card.innerText.includes('课程冲突')) hasConflict = true;
            if (hasConflict) continue;

            let full = false;
            const selectedCount = findFieldValueByLabel(card, '已选人数');
            const capacityCount = findFieldValueByLabel(card, '课容量');
            if (selectedCount && capacityCount) {
                full = isFull(selectedCount, capacityCount);
            } else {
                const text = normalizeText(card.textContent);
                const oldMatch = text.match(/已选\s*\/\s*容量[:：]?\s*(\d+)\s*\/\s*(\d+)/);
                if (oldMatch) {
                    full = isFull(oldMatch[1], oldMatch[2]);
                }
            }
            if (full) continue;

            const btns = card.querySelectorAll('button.el-button--primary');
            for (let b of btns) {
                if (normalizeText(b.innerText).includes('选择') && !b.disabled) {
                    const candidate = {
                        btn: b,
                        name: courseName,
                        matchCount,
                        totalVisible,
                        matchedKeywordIndex,
                        matchedKeyword: matchedKeywordIndex >= 0 ? keywords[matchedKeywordIndex] : ''
                    };

                    if (!bestCandidate) {
                        bestCandidate = candidate;
                    } else {
                        const currentScore = candidate.matchedKeywordIndex < 0 ? Number.MAX_SAFE_INTEGER : candidate.matchedKeywordIndex;
                        const bestScore = bestCandidate.matchedKeywordIndex < 0 ? Number.MAX_SAFE_INTEGER : bestCandidate.matchedKeywordIndex;
                        if (currentScore < bestScore) {
                            bestCandidate = candidate;
                        }
                    }
                    break;
                }
            }
        }

        if (bestCandidate) {
            bestCandidate.matchCount = matchCount;
            bestCandidate.totalVisible = totalVisible;
            return bestCandidate;
        }

        return { btn: null, matchCount, totalVisible, matchedKeyword: '', matchedKeywordIndex: -1 };
    }

    // Step 4C) 统一入口：先表格后卡片
    function findBestLesson(keywords = getKeywordList()) {
        const tableResult = findBestLessonFromTable(keywords);
        if (tableResult.totalVisible > 0) return tableResult;

        return findBestLessonFromCards(keywords);
    }

    // Step 5) 自动确认弹窗
    function tryConfirmDialog() {
        const confirmBtn = document.querySelector('.el-message-box__btns .el-button--primary');
        if (confirmBtn && confirmBtn.offsetParent !== null) {
            const now = Date.now();
            if (now - lastConfirmClickTime < confirmClickCooldown) {
                return true;
            }
            lastConfirmClickTime = now;
            log('检测到确认弹窗，正在点击...', 'success');
            confirmBtn.click();
            return true;
        }
        return false;
    }

    // Step 6) 主循环（50ms）
    function taskTick() {
        if (!isRunning) return;

        const maxRetry = parseInt(retryInput.value) || 50;
        const startTime = new Date(startTimeInput.value).getTime();
        const nowTime = Date.now();

        if (nowTime < startTime) {
            const diff = Math.ceil((startTime - nowTime) / 1000);
            statusEl.textContent = `⏳ 倒计时: ${diff} 秒`;
            statusEl.className = 'nscg-status waiting';
            return; 
        }

        const activeKeywords = getActiveKeywords();
        const activeKeyword = activeKeywords[0] || '';
        statusEl.textContent = activeKeyword
            ? `🚀 抢课中... [${currentKeywordIndex + 1}/${runKeywords.length}] ${activeKeyword} (尝试: ${retryCount}/${maxRetry})`
            : `🚀 抢课中... (尝试: ${retryCount}/${maxRetry})`;
        statusEl.className = 'nscg-status';
        
        // Step 6.1: 达到重试上限停止
        if (retryCount >= maxRetry) {
            if (!switchToNextKeyword(`已达最大次数`)) {
                stop('达到最大重试次数');
            }
            return;
        }

        // Step 6.2: 刷新本轮不继续点击
        const fastModeResult = tryApplyFastModePageSize();
        if (fastModeResult.refreshed) {
            suppressStrictUntil = Date.now() + 1500;
            return;
        }

        // Step 6.3: 优先处理确认弹窗，防止卡在二次确认
        if (tryConfirmDialog()) {
             // retryCount++; // 移除此处计数：弹窗确认不消耗重试次数，防止因页面响应滞后导致的计数瞬间耗尽
             return;
        }

        const result = findBestLesson(activeKeywords);
        const hasKeywords = activeKeywords.length > 0;

        // Step 6.4: 无可点击课程时，也按冷却节奏推进重试，避免“看起来卡住”
        if (!result.btn) {
            const cooldown = parseInt(intervalInput.value) || 1500;
            if (nowTime - lastIdleTickTime >= cooldown) {
                const isNoMatchSearching = hasKeywords && result.matchCount === 0;
                if (!isNoMatchSearching) {
                    retryCount++;
                }
                lastIdleTickTime = nowTime;

                if (nowTime - lastProgressLogTime >= 5000) {
                    if (hasKeywords && result.matchCount > 0) {
                        log(`关键词已命中 ${result.matchCount} 门，但当前均不可选`);
                    } else {
                        log('暂未发现可选课程，继续重试...');
                    }
                    lastProgressLogTime = nowTime;
                }
            }
        }

        // Step 6.5: 命中目标后执行点击；仅限制“连续点击间隔”
        if (result.btn) {   
            const cooldown = parseInt(intervalInput.value) || 1500;
            if (nowTime - lastActionTime < cooldown) {
                return; // 冷却中，跳过
            }

            if (result.matchedKeyword) {
                log(`发现: ${result.name}（${result.matchedKeyword}）`, 'success');
            } else {
                log(`发现: ${result.name}`, 'success');
            }
            result.btn.click();
            retryCount++;
            lastActionTime = Date.now(); // 更新操作时间
            return;
        }

        // Step 6.6: 关键词无命中停止
        if (hasKeywords) {
            if (Date.now() < suppressStrictUntil) {
                return;
            }
            if (result.totalVisible > 0 && result.matchCount === 0) {
                if (!switchToNextKeyword('当前关键词无匹配')) {
                    stop(`未找到匹配关键词课程（${runKeywords.join(' / ')}），已停止`);
                }
                return;
            }
        }
    }

    // --- Control Functions ---
    function toggle() {
        if (isRunning) {
            stop('用户手动停止');
        } else {
            start();
        }
    }

    function start() {
        isRunning = true;
        retryCount = 0;
        runKeywords = getKeywordList();
        currentKeywordIndex = runKeywords.length > 0 ? 0 : -1;
        lastActionTime = 0; // 重置冷却计时
        lastIdleTickTime = 0;
        lastFastModeApplyTime = 0;
        lastConfirmClickTime = 0;
        lastProgressLogTime = 0;
        suppressStrictUntil = Date.now() + 1200;
        
        toggleBtn.innerHTML = '🛑 停止抢课';
        toggleBtn.classList.add('stop');
        
        log('======== 脚本启动 ========', 'success');
        saveConfig();
        const startTime = new Date(startTimeInput.value);
        log(`计划开始: ${startTime.toLocaleTimeString()}`);
        log(`关键词: ${runKeywords.length > 0 ? runKeywords.join(' / ') : '无'}`);
        if (fastModeInput.checked) {
            log(`极速模式: 开启（目标每页 ${Math.max(10, parseInt(pageSizeInput.value, 10) || 200)} 条）`);
        } else {
            log('极速模式: 关闭');
        }
        log('======== 开始抢课 ========', 'success');
        if (runKeywords.length > 0) {
            log(`当前关键词: ${runKeywords[0]}`);
        }
        
        // 脚本的扫描频率固定为 50ms (极速扫描)
        // 实际的点击频率由 taskTick 内部的冷却逻辑(intervalInput)控制
        timer = setInterval(taskTick, 50);
    }

    function stop(reason) {
        isRunning = false;
        clearInterval(timer);
        timer = null;
        
        toggleBtn.innerHTML = '🔥 开始抢课';
        toggleBtn.classList.remove('stop');
        statusEl.textContent = reason || '已停止';
        statusEl.className = 'nscg-status waiting';
        
        log(`任务结束: ${reason}`, 'error');
    }

    // Bind Events
    toggleBtn.addEventListener('click', toggle);

})();
