// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 批量工作台：一个文件夹进、一个文件夹出，纯本机批处理，和 agent 会话互补那一半。
// 会话是「一次一两张、一个模型盯着」，这里是「一个 token 都不花」的确定性重复劳动，
// 所以面板里没有任何模型控件：两个文件夹、一遍扫描、一个开始，就是全部。
// 过程走 batch-event，逐文件折进行表；跑完的回执带成败计数，失败原因跟在自己那一行上。

import {
  Alert,
  Button,
  Dropdown,
  Input,
  InputNumber,
  Popconfirm,
  Progress,
  Segmented,
  Select,
  Switch,
} from "antd";
import { FileDown, FolderOpen, Layers, Play, Save, ScanSearch, Trash2, X } from "lucide-react";
import { open, save } from "@tauri-apps/plugin-dialog";

import { Field, QuantizeFields } from "./QuantizeFields";
import {
  MAX_RECIPE_NAME_CHARS,
  batchPercent,
  canRunBatch,
  recipeImportNotables,
  recipeNameProblem,
  tallyRecipeImport,
  scanMatchesKind,
} from "../lib/batch";
import { useStore } from "../lib/store";
import { useT } from "../lib/t";
import type { BatchKind, ExportFormat } from "../lib/types";

function baseName(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

/** 文件夹行：选好了显示目录名（悬停看全路径），空着就是一个选择按钮。 */
function DirField({
  label,
  value,
  onPick,
  onClear,
}: {
  label: string;
  value: string;
  onPick: () => void;
  onClear: () => void;
}) {
  const t = useT();
  if (value !== "") {
    return (
      <Field label={label}>
        <div className="dock-path">
          <span className="grow" title={value}>
            {baseName(value)}
          </span>
          <Button
            size="small"
            type="text"
            aria-label={t("batch.clear")}
            icon={<X size={12} />}
            onClick={onClear}
          />
        </div>
      </Field>
    );
  }
  return (
    <Field label={label}>
      <Button size="small" block icon={<FolderOpen size={12} />} onClick={onPick}>
        {t("batch.pick")}
      </Button>
    </Field>
  );
}

export default function BatchPanel() {
  const t = useT();
  const recipe = useStore((s) => s.recipe);
  const scan = useStore((s) => s.scan);
  const scanBusy = useStore((s) => s.scanBusy);
  const run = useStore((s) => s.run);
  const setBatchKind = useStore((s) => s.setBatchKind);
  const patchBatchRecipe = useStore((s) => s.patchBatchRecipe);
  const pickBatchInput = useStore((s) => s.pickBatchInput);
  const pickBatchOutput = useStore((s) => s.pickBatchOutput);
  const scanBatchInput = useStore((s) => s.scanBatchInput);
  const runBatch = useStore((s) => s.runBatch);
  const resetBatch = useStore((s) => s.resetBatch);
  const recipeBook = useStore((s) => s.recipeBook);
  const recipeName = useStore((s) => s.recipeName);
  const recipeBusy = useStore((s) => s.recipeBusy);
  const patchRecipeName = useStore((s) => s.patchRecipeName);
  const saveRecipeAs = useStore((s) => s.saveRecipeAs);
  const applyRecipe = useStore((s) => s.applyRecipe);
  const removeRecipe = useStore((s) => s.removeRecipe);
  const recipeImport = useStore((s) => s.recipeImport);
  const exportRecipes = useStore((s) => s.exportRecipes);
  const importRecipes = useStore((s) => s.importRecipes);

  // store 会在 kind / 目录变化时作废旧扫描，这里再确认一次：清单和 recipe 必须是同一码事。
  const scanned = scan !== null && scanMatchesKind(scan, recipe.kind);
  const canRun = scanned && canRunBatch(scan, run);
  // 名字合不合法前端先算一遍（Rust 侧还有一道），理由直接摆在按钮旁边。
  const problem = recipeNameProblem(recipeName, recipeBook);
  const problemVars =
    problem === "long"
      ? { count: MAX_RECIPE_NAME_CHARS }
      : problem === "full"
        ? { count: recipeBook.length }
        : undefined;
  const savedName = recipeBook.some((entry) => entry.name === recipeName);

  /** 配方的进出只认 `.aipr`：伸手给对方的就是一个能在任何编辑器里读的 JSON。 */
  const aiprFilter = [{ name: t("dialog.aipr"), extensions: ["aipr"] }];

  /**
   * 导出手上这份：调完参数直接分享，不必先存进本机簿子。
   * 名字借用配方名输入框——空着就没法分享，占位符已经说了要起名。
   */
  async function exportCurrentRecipe() {
    const name = recipeName.trim();
    if (name === "") return;
    const target = await save({ defaultPath: `${name}.aipr`, filters: aiprFilter });
    if (typeof target !== "string") return;
    await exportRecipes([{ name, recipe }], target);
  }

  /** 整本搬走：换机器、给同事，一个文件就是全部家当。 */
  async function exportWholeBook() {
    if (recipeBook.length === 0) return;
    const target = await save({ defaultPath: "recipes.aipr", filters: aiprFilter });
    if (typeof target !== "string") return;
    await exportRecipes(recipeBook, target);
  }

  async function importRecipeFile() {
    const picked = await open({ multiple: false, filters: aiprFilter });
    if (typeof picked !== "string") return;
    await importRecipes(picked);
  }

  async function pickDir(assign: (dir: string) => void) {
    const picked = await open({ directory: true });
    if (typeof picked === "string") assign(picked);
  }

  return (
    <section className="panel dock">
      <div className="panel-head">
        <Layers size={13} />
        <strong>{t("batch.title")}</strong>
      </div>

      <div className="panel-body dock-body">
        <div className="dock-panel">
          <p className="dock-note">{t("batch.blurb")}</p>

          <Field label={t("batch.kind")}>
            <Segmented
              size="small"
              block
              value={recipe.kind}
              options={[
                {
                  label: t("batch.kind.quantize"),
                  value: "quantize",
                  title: t("batch.kind.quantize.hint"),
                },
                {
                  label: t("batch.kind.export"),
                  value: "export",
                  title: t("batch.kind.export.hint"),
                },
              ]}
              onChange={(next) => setBatchKind(next as BatchKind)}
            />
          </Field>

          <Field label={t("batch.recipe.name")}>
            <div className="dock-actions">
              <Input
                size="small"
                style={{ flex: 1, minWidth: 0 }}
                value={recipeName}
                maxLength={MAX_RECIPE_NAME_CHARS + 8}
                placeholder={t("batch.recipe.name_placeholder")}
                onChange={(event) => patchRecipeName(event.target.value)}
              />
              <Button
                size="small"
                type="primary"
                icon={<Save size={12} />}
                loading={recipeBusy}
                disabled={problem !== null}
                onClick={() => void saveRecipeAs()}
              >
                {t("batch.recipe.save")}
              </Button>
            </div>
            {/* 空白初始态不唠叨：占位符已经说了要起名。存完盘输入框清空，也归这片安静。 */}
            {problem && recipeName.length > 0 ? (
              <p className="dock-note">{t(`batch.recipe.problem.${problem}`, problemVars)}</p>
            ) : null}
          </Field>

          {recipeBook.length > 0 ? (
            <Field label={t("batch.recipe.book")}>
              <div className="dock-actions">
                <Select
                  size="small"
                  style={{ flex: 1, minWidth: 0 }}
                  value={savedName ? recipeName : undefined}
                  placeholder={t("batch.recipe.pick")}
                  options={recipeBook.map((entry) => ({
                    value: entry.name,
                    label: entry.name,
                  }))}
                  onChange={(next) => applyRecipe(next)}
                />
                <Popconfirm
                  title={t("batch.recipe.confirm_delete", { name: recipeName })}
                  okText={t("batch.recipe.delete_ok")}
                  cancelText={t("batch.recipe.delete_cancel")}
                  okButtonProps={{ danger: true }}
                  disabled={!savedName}
                  onConfirm={() => void removeRecipe(recipeName)}
                >
                  <Button
                    size="small"
                    type="text"
                    danger
                    disabled={!savedName}
                    icon={<Trash2 size={12} />}
                  />
                </Popconfirm>
              </div>
            </Field>
          ) : (
            <p className="dock-note">{t("batch.recipe.empty")}</p>
          )}

          <Field label={t("batch.recipe.file")}>
            <div className="dock-actions">
              <Dropdown
                trigger={["click"]}
                menu={{
                  items: [
                    {
                      key: "current",
                      label: t("batch.recipe.export_one"),
                      // 名字借用配方名输入框：空着就没法分享，占位符已经说了要起名。
                      disabled: recipeName.trim() === "",
                    },
                    {
                      key: "all",
                      label: t("batch.recipe.export_all"),
                      disabled: recipeBook.length === 0,
                    },
                    { type: "divider" },
                    { key: "import", label: t("batch.recipe.import") },
                  ],
                  onClick: ({ key }) => {
                    if (key === "current") void exportCurrentRecipe();
                    else if (key === "all") void exportWholeBook();
                    else void importRecipeFile();
                  },
                }}
              >
                <Button size="small" icon={<FileDown size={12} />} loading={recipeBusy}>
                  {t("batch.recipe.file.button")}
                </Button>
              </Dropdown>
            </div>

            <p className="dock-note">{t("batch.recipe.file.hint")}</p>

            {recipeImport ? (
              <>
                <p className="dock-note">
                  {t("batch.recipe.import_note", {
                    count: recipeImport.rows.length,
                    ...tallyRecipeImport(recipeImport.rows),
                  })}
                </p>
                {recipeImportNotables(recipeImport.rows).map((row, index) => (
                  <p className="dock-note" key={`${row.name}-${index}`}>
                    {t(
                      row.state === "renamed"
                        ? "batch.recipe.import.row.renamed"
                        : "batch.recipe.import.row.skipped",
                      { name: row.name, final: row.final_name, note: row.note },
                    )}
                  </p>
                ))}
              </>
            ) : null}
          </Field>

          <DirField
            label={t("batch.input")}
            value={recipe.input_dir}
            onPick={() => void pickDir(pickBatchInput)}
            onClear={() => pickBatchInput("")}
          />
          <DirField
            label={t("batch.output")}
            value={recipe.output_dir}
            onPick={() => void pickDir(pickBatchOutput)}
            onClear={() => pickBatchOutput("")}
          />

          <Button
            size="small"
            block
            icon={<ScanSearch size={13} />}
            loading={scanBusy}
            disabled={recipe.input_dir === ""}
            onClick={() => void scanBatchInput()}
          >
            {scanned ? t("batch.rescan") : t("batch.scan")}
          </Button>

          {scanned && scan ? (
            <p className="dock-note">
              {scan.count > 0
                ? t("batch.scan_ready", { count: scan.count })
                : t("batch.scan_empty")}
            </p>
          ) : (
            <p className="dock-note">{t("batch.idle")}</p>
          )}

          {scan?.truncated ? (
            <Alert
              type="warning"
              showIcon
              className="dock-alert"
              message={t("batch.scan_truncated", { count: scan.count })}
            />
          ) : null}

          {recipe.kind === "quantize" ? (
            <>
              <div className="dock-flag">
                <Switch
                  size="small"
                  checked={recipe.match_source_size}
                  onChange={(next) => patchBatchRecipe({ match_source_size: next })}
                />
                <span>{t("batch.match_source")}</span>
              </div>
              {recipe.match_source_size ? null : (
                <Field label={t("batch.target_size")}>
                  <div className="batch-dims">
                    <InputNumber
                      size="small"
                      style={{ flex: 1, minWidth: 0 }}
                      min={1}
                      max={1024}
                      value={recipe.target_w}
                      onChange={(next) => patchBatchRecipe({ target_w: next ?? 64 })}
                    />
                    <span className="batch-dims-x">x</span>
                    <InputNumber
                      size="small"
                      style={{ flex: 1, minWidth: 0 }}
                      min={1}
                      max={1024}
                      value={recipe.target_h}
                      onChange={(next) => patchBatchRecipe({ target_h: next ?? 64 })}
                    />
                  </div>
                </Field>
              )}
              <QuantizeFields
                value={recipe.options}
                onChange={(options) => patchBatchRecipe({ options })}
              />
            </>
          ) : (
            <Field label={t("batch.export_format")}>
              <Segmented
                size="small"
                block
                value={recipe.export_format}
                options={[
                  { label: "PNG", value: "png" },
                  { label: "GIF", value: "gif" },
                ]}
                onChange={(next) => patchBatchRecipe({ export_format: next as ExportFormat })}
              />
            </Field>
          )}

          <Button
            size="small"
            block
            type="primary"
            icon={<Play size={12} />}
            loading={run.running}
            disabled={!canRun}
            onClick={() => void runBatch()}
          >
            {t("batch.run")}
          </Button>
        </div>

        {/* 跑起来才亮进度条：indeterminate 是不是比 fake 百分比好看，
            所以 total 还没数出来时它自己会转圈。 */}
        {run.running ? (
          <div className="dock-panel">
            <Progress percent={batchPercent(run)} size="small" status="active" />
            <p className="dock-probe">
              {t("batch.running", { done: run.done, total: run.total })}
            </p>
          </div>
        ) : null}

        {/* 逐文件回执留着不自动清：跑完用户要的就是「谁成了谁败了」，
            清掉这张表等于让他再跑一遍才看得见。 */}
        {run.rows.length > 0 ? (
          <div className="batch-rows">
            {run.rows.map((row, index) => (
              <div key={`${row.file}-${index}`} className={`batch-row ${row.state}`}>
                <span className="batch-row-state">{t(`batch.state.${row.state}`)}</span>
                <span className="batch-row-name" title={row.file}>
                  {row.file}
                </span>
                <span className="batch-row-note" title={row.note}>
                  {row.note}
                </span>
              </div>
            ))}
          </div>
        ) : null}

        {/* 计数说三种结局：成、跳、败。跳过单列是因为跳不算失败——
            不匹配的文件本来就不该搅进成败统计里。 */}
        {run.summary ? (
          <div className="dock-result">
            <div className="dock-result-summary">
              {t(run.summary.failed > 0 || run.summary.skipped > 0 ? "batch.done" : "batch.done_clean", {
                ok: run.summary.ok,
                skipped: run.summary.skipped,
                failed: run.summary.failed,
              })}
            </div>
            <p className="dock-note" title={run.summary.outputDir}>
              {t("batch.output_to", { dir: run.summary.outputDir })}
            </p>
            <div className="dock-actions">
              <Button size="small" onClick={resetBatch}>
                {t("batch.clear")}
              </Button>
            </div>
          </div>
        ) : null}

        {run.error ? (
          <Alert type="error" showIcon className="dock-alert" message={run.error} />
        ) : null}
      </div>
    </section>
  );
}
