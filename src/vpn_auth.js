(() => {
  if (window.top !== window) return;
  const install = () => {
    if (!document.body || document.getElementById('dnui-vpn-auth-bar')) return;
    const bar = document.createElement('div');
    bar.id = 'dnui-vpn-auth-bar';
    bar.style.cssText = 'position:fixed;bottom:12px;left:12px;right:12px;z-index:2147483647;padding:14px;background:#eef5ff;color:#183550;border:2px solid #3478f6;border-radius:8px;font:15px sans-serif;box-shadow:0 2px 10px #888';
    const note = document.createElement('span');
    note.textContent = 'DNUI 独立授权窗口：完成校园 VPN 认证后，返回选课页。会话仅用于本次运行，不读取常用浏览器数据。 ';
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
    document.body.appendChild(bar);
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', install);
  else install();
})();
