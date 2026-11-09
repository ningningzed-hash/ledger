import { createContext, useContext, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ApiContext, createApi } from "./api";
import App from "./App";

export type User = { id: number; username: string };
type Session = { user: User; token: string; warning: string | null };
type Status = { has_users: boolean; has_legacy_data: boolean };
const SESSION_KEY = "ledger.session";
const AuthContext = createContext<{
  user: User; logout: (switching?: boolean) => Promise<void>; passwordChanged: () => void;
} | null>(null);
export function useAuth() {
  const value = useContext(AuthContext);
  if (!value) throw new Error("请先登录");
  return value;
}
const remember = (token: string | null) => {
  try { if (token) sessionStorage.setItem(SESSION_KEY, token); else sessionStorage.removeItem(SESSION_KEY); } catch { /* 会话仍可在内存使用 */ }
};

export default function AuthRoot() {
  const [session, setSession] = useState<Session | null>(null);
  const sessionRef = useRef(session);
  sessionRef.current = session;
  const [loading, setLoading] = useState(true);
  const [leaving, setLeaving] = useState(false);
  const [message, setMessage] = useState("");
  const [loginName, setLoginName] = useState("");
  useEffect(() => {
    let alive = true;
    let token: string | null = null;
    try { token = sessionStorage.getItem(SESSION_KEY); } catch { /* 无持久会话 */ }
    if (!token) { setLoading(false); return; }
    invoke<Session>("resume_session", { sessionToken: token }).then((s) => {
      if (alive) setSession(s);
    }).catch(() => {
      if (alive) { remember(null); setMessage("请重新登录以打开你的账本"); }
    }).finally(() => { if (alive) setLoading(false); });
    return () => { alive = false; };
  }, []);
  useEffect(() => { if (!session) document.documentElement.dataset.theme = "system"; }, [session]);
  const api = useMemo(() => session ? createApi(session.token, () => {
    if (sessionRef.current?.token !== session.token) return;
    sessionRef.current = null;
    remember(null); setMessage("登录已失效，请重新登录"); setSession(null);
  }) : null, [session]);
  const loggedIn = (s: Session) => { remember(s.token); setMessage(""); setSession(s); };
  const passwordChanged = () => { remember(null); setSession(null); setMessage("密码已修改，请使用新密码登录"); };
  const logout = async (switching = false) => {
    if (!session) return;
    setLeaving(true);
    try {
      await invoke("logout_user", { sessionToken: session.token });
      remember(null); setSession(null); setLoginName(switching ? "" : session.user.username);
      setMessage(switching ? "输入另一位用户的用户名和密码，打开对应账本" : "已安全退出登录");
    } catch (e) { setMessage(`退出失败：${String(e)}`); }
    finally { setLeaving(false); }
  };
  if (loading || leaving) return <div className="auth-shell"><p role="status">{leaving ? "正在退出当前账本…" : "正在检查登录状态…"}</p></div>;
  if (!session || !api) return <AuthScreen onLogin={loggedIn} notice={message} initialName={loginName} />;
  return <AuthContext.Provider value={{ user: session.user, logout, passwordChanged }}>
    <ApiContext.Provider value={api}>
      <App key={session.token} />
      {(message || session.warning) && <div className="toast" role="status" onClick={() => { setMessage(""); setSession({ ...session, warning: null }); }}>{message || session.warning}</div>}
    </ApiContext.Provider>
  </AuthContext.Provider>;
}

function AuthScreen({ onLogin, notice, initialName }: { onLogin: (s: Session) => void; notice: string; initialName: string }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [mode, setMode] = useState<"login" | "register">("login");
  const [username, setUsername] = useState(initialName);
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [claim, setClaim] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const load = async () => {
    setError("");
    try { const s = await invoke<Status>("auth_status"); setStatus(s); if (!s.has_users) setMode("register"); }
    catch (e) { setError(String(e)); }
  };
  useEffect(() => { load(); }, []);
  const registering = mode === "register";
  return <div className="auth-shell">
    <div className="auth-intro"><div className="brand"><div className="logo">¥</div><span>Ledger</span></div>
      <h1>每个人，<br />都有自己的账本。</h1><p>记录生活，管理资产。<br />登录后继续你的理财日常。</p>
      <div className="auth-points"><span>独立账本</span><span>专属助手</span><span>个人偏好</span></div>
    </div>
    <div className="auth-card profile-card">
      <h2>{registering ? "创建你的用户" : "欢迎回来"}</h2>
      <p className="muted">{registering ? "创建用户名与密码，开启自己的账本。" : "使用本机注册的用户登录。"}</p>
      {notice && <p className="auth-notice" role="status">{notice}</p>}
      {!status ? <><p role={error ? "alert" : "status"}>{error || "正在连接账本…"}</p>{error && <button className="btn alt" onClick={load}>重试连接</button>}</> : <>
        <div className="auth-tabs" role="tablist" aria-label="登录或注册">
          <button role="tab" aria-selected={!registering} disabled={busy} onClick={() => { setMode("login"); setError(""); setPassword(""); setConfirm(""); }}>登录</button>
          <button role="tab" aria-selected={registering} disabled={busy} onClick={() => { setMode("register"); setError(""); setPassword(""); setConfirm(""); }}>注册</button>
        </div>
        <form onSubmit={async (e) => {
          e.preventDefault();setError("");
          if (registering && password !== confirm) { setError("两次输入的密码不一致"); return; }
          setBusy(true);
          try {
            const s = await invoke<Session>(registering ? "register_user" : "login_user", { username: username.trim(), password, ...(registering ? { claimLegacy: claim } : {}) });
            setPassword(""); setConfirm(""); onLogin(s);
          } catch (err) { setError(String(err)); }
          finally { setBusy(false); }
        }}>
          <fieldset className="profile-fields" disabled={busy}>
            <label className="lbl" htmlFor="auth-username">用户名</label>
            <input id="auth-username" className="f" value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="username" required minLength={3} maxLength={32} pattern="[a-zA-Z0-9_\-]{3,32}" placeholder="3–32 位字母、数字、下划线或短横线" autoCapitalize="none" spellCheck={false} autoFocus />
            <label className="lbl" htmlFor="auth-password">密码</label>
            <input id="auth-password" className="f" type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete={registering ? "new-password" : "current-password"} required minLength={registering ? 8 : undefined} maxLength={128} placeholder={registering ? "至少 8 个字符" : "输入密码"} />
            {registering && <><label className="lbl" htmlFor="auth-confirm">确认密码</label><input id="auth-confirm" className="f" type="password" value={confirm} onChange={(e) => setConfirm(e.target.value)} autoComplete="new-password" required maxLength={128} /></>}
            {registering && status.has_legacy_data && <label className="auth-claim"><input type="checkbox" checked={claim} onChange={(e) => setClaim(e.target.checked)} required /><span>我是原账本的使用者，同意将已有账目、资产、聊天记录和设置归入这个首个用户。</span></label>}
            {registering && status.has_users && <p className="muted">新用户拥有独立的空账本，已有用户的数据会保留。</p>}
            {error && <p className="profile-error" role="alert">{error}</p>}
            <button className="btn auth-submit" type="submit">{busy ? "请稍候…" : registering ? "注册并进入账本" : "登录账本"}</button>
          </fieldset>
        </form>
      </>}
    </div>
  </div>;
}

export function AccountSecurity() {
  const auth = useAuth();
  const api = useContext(ApiContext)!;
  const [editing, setEditing] = useState(false);
  const [current, setCurrent] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return <section className="profile-card account-security">
    <div className="account-security-head"><div><h2>登录与安全</h2><p className="muted">当前用户：{auth.user.username} · 账本、聊天与设置独立保存</p></div>
      <div className="row-inline"><button className="btn alt" onClick={() => { setEditing(!editing); setCurrent(""); setPassword(""); setConfirm(""); setError(""); }} disabled={busy}>修改密码</button><button className="btn alt" onClick={() => auth.logout(true)} disabled={busy}>切换用户</button><button className="btn alt" onClick={() => auth.logout()} disabled={busy}>退出登录</button></div>
    </div>
    {editing && <form className="password-form" onSubmit={async (e) => {
      e.preventDefault();setError("");
      if (password !== confirm) { setError("两次输入的密码不一致"); return; }
      setBusy(true);
      try { await api.changePassword(current, password); auth.passwordChanged(); }
      catch (err) { setError(String(err)); }
      finally { setBusy(false); }
    }}>
      <fieldset className="profile-fields" disabled={busy}>
        <label className="lbl" htmlFor="current-password">当前密码</label><input id="current-password" className="f" type="password" value={current} onChange={(e) => setCurrent(e.target.value)} required maxLength={128} autoComplete="current-password" />
        <label className="lbl" htmlFor="new-password">新密码</label><input id="new-password" className="f" type="password" value={password} onChange={(e) => setPassword(e.target.value)} required minLength={8} maxLength={128} autoComplete="new-password" />
        <label className="lbl" htmlFor="confirm-password">确认新密码</label><input id="confirm-password" className="f" type="password" value={confirm} onChange={(e) => setConfirm(e.target.value)} required maxLength={128} autoComplete="new-password" />
        {error && <p className="profile-error" role="alert">{error}</p>}
        <p className="muted">修改后需要使用新密码重新登录。</p><button className="btn" type="submit">{busy ? "修改中…" : "保存新密码"}</button>
      </fieldset>
    </form>}
  </section>;
}
