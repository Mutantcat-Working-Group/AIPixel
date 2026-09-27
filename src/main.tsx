import React from "react";
import ReactDOM from "react-dom/client";
import { ConfigProvider, theme } from "antd";
import zhCN from "antd/locale/zh_CN";

import App from "./App";
import "./styles.css";

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

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ConfigProvider theme={themeConfig} locale={zhCN}>
      <App />
    </ConfigProvider>
  </React.StrictMode>,
);
