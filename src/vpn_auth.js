(() => {
  if (window.top !== window) return;

  const timetablePath = '/jwapp/sys/kbapp/*default/index.do#/wdkb';
  const text = (element) => (element?.textContent || '').replace(/\s+/g, ' ').trim();
  const isTimetableRoute = () => location.pathname.includes('/jwapp/sys/kbapp/');

  const flexUnits = (element) => {
    const match = (element.style.flex || '').match(/^\s*([0-9]+(?:\.[0-9]+)?)/);
    const value = match ? Number(match[1]) : 2;
    return Number.isFinite(value) && value > 0 ? Math.max(1, Math.round(value / 2)) : 1;
  };

  const parseWeeks = (value) => {
    const parity = /单|奇/.test(value) ? 1 : /双|偶/.test(value) ? 2 : 0;
    const ranges = [];
    const pattern = /(\d+)\s*(?:-\s*(\d+))?\s*周/g;
    let match;
    while ((match = pattern.exec(value)) !== null) {
      const start = Number(match[1]);
      const end = Number(match[2] || match[1]);
      if (Number.isInteger(start) && Number.isInteger(end) && start > 0 && end > 0) {
        ranges.push({ start: Math.min(start, end), end: Math.max(start, end), parity });
      }
    }
    return ranges;
  };

  const extractSchedule = () => {
    const container = document.querySelector('.kbappTimetableContentContainer');
    if (!container) throw new Error('请先在教务系统打开“我的课表”');
    const dayRoots = Array.from(container.children).filter((element) =>
      element.classList.contains('kbappTimetableDayColumnRoot')
    );
    if (dayRoots.length !== 7) throw new Error('未识别到完整的星期列，请等待课表加载完成');

    const entries = [];
    const seen = new Set();
    dayRoots.forEach((dayRoot, dayIndex) => {
      let majorIndex = 0;
      Array.from(dayRoot.children).forEach((slot) => {
        const span = flexUnits(slot);
        if (!slot.classList.contains('kbappTimetableDayColumnConflictContainer')) {
          majorIndex += span;
          return;
        }
        const startSection = majorIndex * 2 + 1;
        const endSection = (majorIndex + span) * 2;
        const items = Array.from(slot.querySelectorAll('.kbappTimetableCourseRenderCourseItem'));
        items.forEach((item) => {
          const lines = Array.from(item.children)
            .filter((element) => element.classList.contains('kbappTimetableCourseRenderCourseItemInfoText'))
            .map(text)
            .filter(Boolean);
          const name = lines[0] || '';
          const weeks = lines.slice(1).flatMap(parseWeeks);
          const entry = {
            name,
            weekday: dayIndex + 1,
            start_section: startSection,
            end_section: endSection,
            weeks,
          };
          const key = JSON.stringify(entry);
          if (!seen.has(key)) {
            seen.add(key);
            entries.push(entry);
          }
        });
        majorIndex += span;
      });
    });

    return {
      semester: text(document.querySelector('.kbappTimeXQText')),
      current_week: text(document.querySelector('.kbappTimeZCText')),
      entries,
    };
  };

  const waitForScheduleDom = () => new Promise((resolve, reject) => {
    const deadline = Date.now() + 15000;
    const check = () => {
      const container = document.querySelector('.kbappTimetableContentContainer');
      const dayRoots = container
        ? Array.from(container.children).filter((element) =>
          element.classList.contains('kbappTimetableDayColumnRoot'))
        : [];
      if (container && dayRoots.length === 7) {
        resolve();
      } else if (Date.now() >= deadline) {
        reject(new Error('课表加载超时，请确认“我的课表”页面已显示完整内容'));
      } else {
        window.setTimeout(check, 250);
      }
    };
    check();
  });

  const install = () => {
    if (!document.body || document.getElementById('dnui-vpn-auth-bar')) return;
    const bar = document.createElement('div');
    bar.id = 'dnui-vpn-auth-bar';
    bar.style.cssText = 'position:fixed;bottom:12px;left:12px;right:12px;z-index:2147483647;padding:14px;background:#eef5ff;color:#183550;border:2px solid #3478f6;border-radius:8px;font:15px sans-serif;box-shadow:0 2px 10px #888';
    const note = document.createElement('span');
    note.textContent = 'DNUI 独立授权窗口：完成校园登录后，可返回选课页或导入当前课表。会话仅用于本次运行，不读取常用浏览器数据。 ';
    bar.appendChild(note);
    const link = document.createElement('a');
    link.href = 'https://xk.neusoft.edu.cn/xsxk/profile/index.html';
    link.textContent = '返回选课页';
    link.style.cssText = 'color:#174fa0;margin-right:14px';
    bar.appendChild(link);
    if (location.origin === 'https://xk.neusoft.edu.cn' && location.pathname.startsWith('/xsxk/')) {
      const button = document.createElement('button');
      button.type = 'button';
      button.textContent = '完成授权并检测';
      button.style.cssText = 'padding:8px 14px;background:#2365cf;color:white;border:0;border-radius:5px;cursor:pointer';
      button.onclick = () => {
        button.disabled = true;
        button.textContent = '正在交给桌面程序检测…';
        window.ipc.postMessage('dnui-complete-vpn-auth');
      };
      bar.appendChild(button);
    }
    if (location.origin === 'https://teach.neusoft.edu.cn') {
      const button = document.createElement('button');
      button.type = 'button';
      button.textContent = isTimetableRoute() ? '导入当前课表' : '打开我的课表';
      button.style.cssText = 'padding:8px 14px;background:#2365cf;color:white;border:0;border-radius:5px;cursor:pointer';
      button.onclick = async () => {
        button.disabled = true;
        if (!isTimetableRoute()) {
          note.textContent = '正在打开“我的课表”，页面加载完成后再点击导入。 ';
          window.location.href = timetablePath;
          return;
        }
        button.textContent = '正在等待课表加载…';
        try {
          await waitForScheduleDom();
          button.textContent = '正在读取课表…';
          window.ipc.postMessage(`dnui-import-schedule:${JSON.stringify(extractSchedule())}`);
        } catch (error) {
          button.disabled = false;
          button.textContent = isTimetableRoute() ? '导入当前课表' : '打开我的课表';
          note.textContent = `课表读取失败：${error.message || '请先打开“我的课表”'} `;
        }
      };
      bar.appendChild(button);
    }
    document.body.appendChild(bar);
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', install);
  else install();
})();
