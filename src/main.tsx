import React from "react";
import ReactDOM from "react-dom/client";
import { ConfigProvider, theme } from "antd";
import enUS from "antd/locale/en_US";
import zhCN from "antd/locale/zh_CN";

import App from "./App";
import { useStore } from "./lib/store";
import "./styles.css";

/**
 * 兜底边界：渲染炸了也别让整个窗口变白。WebView 崩了用户只能重开，
 * 这里至少留一句「发生了什么 + 重载」。
 */
class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="gate">
        <div className="gate-inner">
          <h1>AIPixel</h1>
          <p>{this.state.error.message || String(this.state.error)}</p>
          <div>
            <button
              type="button"
              className="crash-reload"
              onClick={() => window.location.reload()}
            >
              Reload
            </button>
          </div>
        </div>
      </div>
    );
  }
}

// 深色工具向基调：主色继承老版本的蓝，暖色留给 agent 活动状态。
const themeConfig = {
  algorithm: theme.darkAlgorithm,
  token: {
    colorPrimary: "#3e9bff",
    colorInfo: "#3e9bff",
    colorBgContainer: "#151b24",
    colorBgElevated: "#1a222d",
    colorBorder: "#232c38",
    colorBorderSecondary: "#232c38",
    colorText: "#e6ebf2",
    colorTextSecondary: "#9aa7b8",
    colorTextTertiary: "#6b7787",
    borderRadius: 6,
    borderRadiusSM: 5,
    fontSize: 13,
    fontFamily:
      '-apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", "Noto Sans SC", sans-serif',
  },
  components: {
    Button: { controlHeightSM: 26, paddingSM: 10 },
    Segmented: { itemSelectedBg: "#1d3a5c", itemColor: "#9aa7b8", itemSelectedColor: "#e6ebf2" },
    Modal: { headerBg: "#151b24", contentBg: "#151b24" },
  },
};

/** antd 自己的文案（分页、空描述）也跟着界面语言走。 */
function LocaleShell() {
  const lang = useStore((s) => s.lang);
  React.useEffect(() => {
    document.documentElement.lang = lang === "zh" ? "zh-CN" : "en";
  }, [lang]);
  return (
    <ConfigProvider theme={themeConfig} locale={lang === "zh" ? zhCN : enUS}>
      <App />
    </ConfigProvider>
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <LocaleShell />
    </ErrorBoundary>
  </React.StrictMode>,
);
