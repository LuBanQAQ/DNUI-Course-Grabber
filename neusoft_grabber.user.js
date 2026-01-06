// ==UserScript==
// @name         Neusoft Course Grabber
// @namespace    http://tampermonkey.net/
// @version      1.0
// @description  东软自动抢课脚本 - 自动识别无冲突课程，支持定时抢课、自动确认、重试机制
// @author       LuBanQAQ
// @match        *://xk.neusoft.edu.cn/xsxk/elective/*
// @grant        none
// @run-at       document-end
// ==/UserScript==

(function() {
    'use strict';

    // --- GUI CSS ---
    const css = `
        #nscg-panel {
            position: fixed;
            top: 50px;
            right: 50px;
            width: 340px;
            background: rgba(255, 255, 255, 0.98);
            border: 1px solid #dcdfe6;
            box-shadow: 0 4px 16px rgba(0, 0, 0, 0.15);
            border-radius: 8px;
            z-index: 99999;
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            font-size: 14px;
            color: #303133;
            display: flex;
            flex-direction: column;
            transition: opacity 0.3s;
        }
        #nscg-header {
            padding: 12px 15px;
            background: linear-gradient(135deg, #409EFF, #3a8ee6);
            color: white;
            font-weight: 600;
            border-radius: 8px 8px 0 0;
            cursor: move;
            display: flex;
            justify-content: space-between;
            align-items: center;
            user-select: none;
        }
        #nscg-content {
            padding: 15px;
            background: #fff;
            border-radius: 0 0 8px 8px;
        }
        .nscg-row {
            margin-bottom: 12px;
            display: flex;
            align-items: center;
        }
        .nscg-label {
            width: 90px;
            font-size: 13px;
            color: #606266;
        }
        .nscg-input {
            flex: 1;
            padding: 6px 10px;
            border: 1px solid #dcdfe6;
            border-radius: 4px;
            outline: none;
            font-size: 13px;
            transition: border-color 0.2s;
        }
        .nscg-input:focus {
            border-color: #409EFF;
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
            margin-top: 5px;
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
        #nscg-log {
            margin-top: 15px;
            height: 180px;
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
            margin-bottom: 12px;
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
            <span>东软抢课助手 Pro</span>
            <span style="font-size: 16px; cursor: pointer; padding: 0 5px;" id="nscg-min" title="最小化">-</span>
        </div>
        <div id="nscg-content">
            <div class="nscg-status waiting" id="nscg-status">等待配置...</div>
            
            <div class="nscg-row">
                <span class="nscg-label">开始时间:</span>
                <input type="datetime-local" id="nscg-start-time" class="nscg-input">
            </div>
            <div class="nscg-row">
                <span class="nscg-label">课程关键词:</span>
                <input type="text" id="nscg-keyword" class="nscg-input" placeholder="为空则抢任意, 如: 排球">
            </div>
            <div class="nscg-row">
                 <label style="cursor:pointer;display:flex;align-items:center;font-size:12px;color:#606266;flex:1;color:#F56C6C;">
                    <input type="checkbox" id="nscg-trap">
                    <span style="margin-left:4px;">🛡️ 拦截跳转(调试)</span>
                </label>
            </div>
            <div class="nscg-row">
                <span class="nscg-label">最大重试:</span>
                <input type="number" id="nscg-retry" class="nscg-input" value="100" min="1">
            </div>
             <div class="nscg-row">
                <span class="nscg-label">点击冷却(ms):</span>
                <input type="number" id="nscg-interval" class="nscg-input" value="1500" min="1" step="100" placeholder="点击后的等待时间">
            </div>
            <div class="nscg-row">
                 <label style="cursor:pointer;display:flex;align-items:center;font-size:12px;color:#606266;flex:1;">
                    <input type="checkbox" id="nscg-keepalive" checked>
                    <span style="margin-left:4px;">WebVPN保活(分):</span>
                    <input type="number" id="nscg-keepalive-interval" class="nscg-input" value="3" min="1" style="margin-left:5px;width:50px;">
                </label>
            </div>
            
            <button id="nscg-toggle-btn" class="nscg-btn">🔥 开始抢课</button>
            <button id="nscg-clear-btn" class="nscg-btn" style="background:#909399;margin-top:8px;font-size:12px;padding:6px;">清空日志</button>
            
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
    let lastActionTime = 0; // 防止操作过快
    
    // Elements
    const statusEl = document.getElementById('nscg-status');
    const toggleBtn = document.getElementById('nscg-toggle-btn');
    const clearBtn = document.getElementById('nscg-clear-btn');
    const logEl = document.getElementById('nscg-log');
    
    const startTimeInput = document.getElementById('nscg-start-time');
    const retryInput = document.getElementById('nscg-retry');
    const intervalInput = document.getElementById('nscg-interval');
    const keywordInput = document.getElementById('nscg-keyword');
    const trapInput = document.getElementById('nscg-trap');
    const keepAliveCheck = document.getElementById('nscg-keepalive');
    const keepAliveInput = document.getElementById('nscg-keepalive-interval');

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
            keyword: keywordInput.value,
            trap: trapInput.checked,
            retry: retryInput.value,
            interval: intervalInput.value,
            keepAlive: keepAliveCheck.checked,
            keepAliveInterval: keepAliveInput.value
        };
        localStorage.setItem('nscg_config', JSON.stringify(config));
    }

    function loadConfig() {
        const saved = localStorage.getItem('nscg_config');
        if (saved) {
            try {
                const c = JSON.parse(saved);
                if(c.start) startTimeInput.value = c.start;
                if(c.keyword !== undefined) keywordInput.value = c.keyword;
                if(c.trap !== undefined) trapInput.checked = c.trap;
                if(c.retry) retryInput.value = c.retry;
                if(c.interval) intervalInput.value = c.interval;
                if(c.keepAlive !== undefined) keepAliveCheck.checked = c.keepAlive;
                if(c.keepAliveInterval) keepAliveInput.value = c.keepAliveInterval;
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
        logEl.prepend(div);
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
        if(e.target.id === 'nscg-min') return;
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
    document.getElementById('nscg-min').addEventListener('click', () => {
        const content = document.getElementById('nscg-content');
        if (content.style.display === 'none') {
            content.style.display = 'block';
            document.getElementById('nscg-min').textContent = '-';
        } else {
            content.style.display = 'none';
            document.getElementById('nscg-min').textContent = '□';
        }
    });

    // --- Core Logic ---

    // 1. Check Capacity (Example: "已选/容量：0/40")
    function isFull(text) {
        const match = text.match(/已选\/容量：\s*(\d+)\s*\/\s*(\d+)/);
        if (match) {
            const selected = parseInt(match[1], 10);
            const capacity = parseInt(match[2], 10);
            return selected >= capacity;
        }
        return false;
    }

    // 2. Find Available Lesson Button
    function findBestLesson() {
        const cards = document.querySelectorAll('.el-card.jxb-card');
        const keyword = keywordInput.value.trim();
        let matchCount = 0;
        
        for (let i = 0; i < cards.length; i++) {
            const card = cards[i];
            
            // Keyword Match
            const nameEl = card.querySelector('.one-row');
            const courseName = nameEl ? nameEl.innerText.trim() : '未知课程';
            const matchesKeyword = !keyword || courseName.includes(keyword);
            
            if (matchesKeyword) matchCount++;
            else continue; 

            // Conflict Check
            const tags = card.querySelectorAll('.el-tag--danger');
            let hasConflict = false;
            for(let t of tags) {
                if (t.innerText.includes('冲突')) { hasConflict = true; break; }
            }
            if (!hasConflict && card.innerText.includes('课程冲突')) hasConflict = true;
            if (hasConflict) continue;

            // Capacity Check
            const items = card.querySelectorAll('.card-item');
            let full = false;
            for(let item of items) {
                if (item.innerText.includes('已选/容量')) {
                    if (isFull(item.innerText)) { full = true; break; }
                }
            }
            if (full) continue;

            // Find Button
            const btns = card.querySelectorAll('button.el-button--primary');
            for (let b of btns) {
                if (b.innerText.trim() === '选择') {
                    return { btn: b, name: courseName, matchCount, totalVisible: cards.length };
                }
            }
        }
        return { btn: null, matchCount, totalVisible: cards.length };
    }

    // 3. Handle Auto-Confirm Dialog
    function tryConfirmDialog() {
        const confirmBtn = document.querySelector('.el-message-box__btns .el-button--primary');
        if (confirmBtn && confirmBtn.offsetParent !== null) {
            log('检测到确认弹窗，正在点击...', 'success');
            confirmBtn.click();
            return true;
        }
        return false;
    }

    // 4. Timer Tick
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

        statusEl.textContent = `🚀 抢课中... (尝试: ${retryCount}/${maxRetry})`;
        statusEl.className = 'nscg-status';
        
        if (retryCount >= maxRetry) {
            stop('达到最大重试次数');
            return;
        }

        // 优先处理弹窗 (不限制频率，确保极速响应)
        if (tryConfirmDialog()) {
             // retryCount++; // 移除此处计数：弹窗确认不消耗重试次数，防止因页面响应滞后导致的计数瞬间耗尽
             return;
        }

        const result = findBestLesson();
        
        // Strict Mode: 如果设置了关键词，则找不到时强制停止
        if (keywordInput.value) {
             if (result.totalVisible > 0 && result.matchCount === 0) {
                 stop(`未找到 "${keywordInput.value}"，已停止`);
                 return;
             }
        }

        if (result.btn) {
            // --- 冷却检测 ---
            // 用户设定的“点击冷却”仅限制连续点击之间的间隔
            // 如果是第一次发现（或者距离上次点击已经过了很久），则立即点击（无延迟）
            const cooldown = parseInt(intervalInput.value) || 1500;
            if (nowTime - lastActionTime < cooldown) {
                return; // 冷却中，跳过
            }

            log(`发现可用: ${result.name}`, 'success');
            result.btn.click();
            retryCount++;
            lastActionTime = Date.now(); // 更新操作时间
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
        lastActionTime = 0; // 重置冷却计时
        
        toggleBtn.innerHTML = '🛑 停止抢课';
        toggleBtn.classList.add('stop');
        
        log('======== 脚本启动 ========', 'success');
        saveConfig();
        const startTime = new Date(startTimeInput.value);
        log(`计划开始: ${startTime.toLocaleTimeString()}`);
        log(`关键词: ${keywordInput.value || '无'}`);
        
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

    // --- WebVPN Keep Alive ---
    function keepAliveTick() {
        if (!keepAliveCheck.checked) return;

        const intervalMinutes = parseFloat(keepAliveInput.value) || 3;
        const intervalMs = intervalMinutes * 60 * 1000;
        
        // 1. 临近抢课不刷新 (例如开始前 5 分钟)
        if (startTimeInput.value) {
            const startTime = new Date(startTimeInput.value).getTime();
            const now = Date.now();
            const diff = startTime - now;
             
            // 如果距离开始时间小于 5 分钟，且还没过开始时间，则暂停刷新
            if (diff > 0 && diff < 5 * 60 * 1000) {
                 return; // 马上要开始了，保持静止
            }
            // 如果已经开始抢课，也暂停
            if (isRunning) return; 
        }

        // 2. 检查上次刷新时间
        const lastReloadStr = localStorage.getItem('nscg_last_reload');
        const now = Date.now();
        let lastReload = lastReloadStr ? parseInt(lastReloadStr) : 0;
        
        // 如果距离上次刷新超过了设定间隔 (且页面至少活跃了10秒，防止无限循环刷新)
        if (now - lastReload > intervalMs) {
            // 安全检查：如果页面刚刚加载不到10秒，不要再次刷新，可能是逻辑错误
            if (performance.now() < 10000) return;

            log('🔄 [WebVPN保活] 页面即将刷新...', 'info');
            saveConfig();
            localStorage.setItem('nscg_last_reload', now);
            setTimeout(() => window.location.reload(), 1000);
        }
    }
    
    // 启动保活定时器 (30秒检查一次)
    setInterval(keepAliveTick, 30 * 1000);
    // 脚本加载后延迟检查一次
    setTimeout(keepAliveTick, 5000);

    // Bind Events
    toggleBtn.addEventListener('click', toggle);

})();
