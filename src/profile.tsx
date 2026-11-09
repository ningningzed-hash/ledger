import { AccountSecurity } from "./auth";
import { useEffect, useState } from "react";
import { useApi, UserProfile } from "./api";

export function ProfileView({ profile, error, onRetry, onSaved, onSettings, pages }: {
  profile: UserProfile | null; error: string; onRetry: () => void;
  onSaved: (p: UserProfile) => void; onSettings: () => void;
  pages: { key: string; label: string }[];
}) {
  const api = useApi();
  const [draft, setDraft] = useState(profile);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [failed, setFailed] = useState(false);
  useEffect(() => { setDraft(profile); }, [profile]);
  const change = <K extends keyof UserProfile>(key: K, value: UserProfile[K]) => {
    setDraft((p) => p && ({ ...p, [key]: value }));
    setMessage("");
  };

  return <div className="profile-page">
    <div className="head"><div><h1>个人中心</h1><p className="muted">管理你的个人资料与使用偏好</p></div></div>
    {!draft ? <div className="profile-card" role={error ? "alert" : "status"}>
      <p>{error || "正在加载个人资料…"}</p>
      {error && <button className="btn alt" onClick={onRetry}>重新加载</button>}
    </div> : <>
      <section className="profile-card profile-summary">
        <div className="profile-avatar" aria-hidden="true">{Array.from(profile?.nickname || "我")[0]}</div>
        <div><h2>{profile?.nickname}</h2><p className="muted">{profile?.bio || "从记录每一笔开始，照顾好自己的生活。"}</p><span className="profile-badge">本地个人账本</span></div>
      </section>
      <form onSubmit={async (e) => {
        e.preventDefault(); setBusy(true); setMessage(""); setFailed(false);
        try {
          const saved = await api.saveProfile(draft);
          onSaved(saved); setDraft(saved); setMessage("已保存，偏好设置已生效");
        } catch (err) { setFailed(true); setMessage(String(err)); }
        finally { setBusy(false); }
      }}>
        <fieldset disabled={busy} className="profile-fields">
          <div className="profile-grid">
            <section className="profile-card">
              <h2>个人资料</h2>
              <label className="lbl" htmlFor="profile-name">昵称</label>
              <input id="profile-name" className="f" value={draft.nickname} maxLength={30} required autoComplete="nickname" onChange={(e) => change("nickname", e.target.value)} />
              <label className="lbl" htmlFor="profile-email">邮箱 <span className="muted">· 选填</span></label>
              <input id="profile-email" className="f" type="email" maxLength={254} value={draft.email} autoComplete="email" placeholder="name@example.com" onChange={(e) => change("email", e.target.value)} />
              <label className="lbl" htmlFor="profile-bio">个人简介 <span className="muted">· 选填</span></label>
              <textarea id="profile-bio" className="f" rows={3} maxLength={200} value={draft.bio} placeholder="写下你的生活态度或理财小目标" onChange={(e) => change("bio", e.target.value)} />
              <p className="muted">资料保存在当前账本中，邮箱仅用于个人信息展示。</p>
            </section>
            <section className="profile-card">
              <h2>偏好设置</h2>
              <label className="lbl" htmlFor="profile-theme">外观</label>
              <select id="profile-theme" className="f" value={draft.theme} onChange={(e) => change("theme", e.target.value as UserProfile["theme"])}>
                <option value="system">跟随系统</option><option value="light">浅色</option><option value="dark">深色</option>
              </select>
              <label className="lbl" htmlFor="profile-start">默认首页</label>
              <select id="profile-start" className="f" value={draft.start_page} onChange={(e) => change("start_page", e.target.value)}>
                {pages.map((p) => <option key={p.key} value={p.key}>{p.label}</option>)}
              </select>
              <p className="muted">下次打开应用时，自动进入所选页面。</p>
            </section>
          </div>
          <div className="profile-actions">
            <button className="btn" type="submit">{busy ? "保存中…" : "保存修改"}</button>
            <button className="btn alt" type="button" onClick={() => { setDraft(profile); setMessage(""); }}>撤销修改</button>
            <span className={failed ? "profile-error" : "muted"} role={failed ? "alert" : "status"}>{message}</span>
          </div>
        </fieldset>
      </form>
    </>}
    <AccountSecurity />
    <section className="profile-card profile-assistant">
      <div><h2>财务助手</h2><p className="muted">配置模型服务、助手人设和理财理念。</p></div>
      <button className="btn alt" onClick={onSettings}>助手设置 →</button>
    </section>
  </div>;
}
