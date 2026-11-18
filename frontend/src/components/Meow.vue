<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { CircleAlert, CircleCheck, Play, RefreshCw, ShieldAlert, Square, Trash2, TriangleAlert } from '@lucide/vue'
import { ZButton } from '@codex-proxy/ui/button'
import { ZSelect } from '@codex-proxy/ui/select'
import { api, meowApi, date } from '../api'
import type { Account, MeowCatalog, MeowRun, MeowSummary } from '../types'

const props = defineProps<{ accounts: Account[]; keys: { id: string; name: string }[]; busy: boolean }>()
const emit = defineEmits<{ error: [message: string]; notice: [message: string] }>()

const catalog = ref<MeowCatalog>()
const runs = ref<MeowSummary[]>([])
const run = ref<MeowRun>()
const loading = ref(true)
const starting = ref(false)
const pumping = ref(false)
const accountId = ref('')
const keyId = ref('')
const requestModel = ref('')
const claimedModel = ref('')
const tier = ref('low')
const models = ref<string[]>([])
const modelsLoading = ref(false)
const stopped = ref(false)
let modelSequence = 0

const accounts = computed(() => props.accounts.filter(a => a.enabled && a.supported && a.provider === 'openai'))
const tierOptions = computed(() => (catalog.value?.tiers ?? []).map(t => ({
  value: t.id,
  label: `${({ low: '快速', medium: '标准', high: '完整' } as Record<string, string>)[t.id] || t.id} · ${t.count} 题`,
})))
const count = computed(() => catalog.value?.tiers.find(t => t.id === tier.value)?.count ?? 0)
const canStart = computed(() => !!accountId.value && !!keyId.value && !!requestModel.value && !!claimedModel.value && !starting.value && !pumping.value)
const finished = computed(() => !!run.value && ['completed', 'stopped'].includes(run.value.status) && !run.value.inflight)
const progress = computed(() => {
  const r = run.value
  if (!r) return 0
  const ok = r.attempts.filter(a => a.status === 'ok').length
  return planned.value ? Math.min(100, Math.round(ok / planned.value * 100)) : 0
})
const planned = computed(() => run.value?.plan.jobs.length ?? 0)
const okCount = computed(() => run.value?.attempts.filter(a => a.status === 'ok').length ?? 0)
const errCount = computed(() => run.value?.attempts.filter(a => a.status === 'error').length ?? 0)
const topMatches = computed(() => {
  const report = run.value?.report
  if (!report?.matches) return []
  return Object.entries(report.matches)
    .map(([model, value]) => ({ model, value, threshold: report.thresholds?.[model] ?? null }))
    .sort((a, b) => b.value - a.value)
})
const statusText = (s: string) => ({ ready: '待继续', running: '检测中', paused: '已暂停', completed: '已完成', stopped: '已结束' }[s] || s)
const verdictText = (v?: string | null) => ({ match: '与对照一致', mismatch: '行为不匹配', insufficient: '证据不足' }[v || ''] || '等待样本')
const verdictTone = (v?: string | null) => v === 'match' ? 'tone-success' : v === 'mismatch' ? 'tone-danger' : 'tone-warning'

async function loadBootstrap() {
  loading.value = true
  try {
    const result = await meowApi.bootstrap()
    catalog.value = result.catalog
    runs.value = result.runs
    if (!claimedModel.value && result.catalog.models.length) claimedModel.value = result.catalog.models[0]!.id
    if (!requestModel.value && result.catalog.models.length) requestModel.value = result.catalog.models[0]!.id
  } catch (e) {
    emit('error', (e as Error).message)
  } finally {
    loading.value = false
  }
}

async function loadModels() {
  const sequence = ++modelSequence
  models.value = []
  requestModel.value = ''
  if (!keyId.value) return
  modelsLoading.value = true
  try {
    const result = await api.models(keyId.value)
    if (sequence === modelSequence) models.value = result.models
  } catch (e) {
    if (sequence === modelSequence) emit('error', (e as Error).message)
  } finally {
    if (sequence === modelSequence) modelsLoading.value = false
  }
}
watch(keyId, loadModels)

function selectRun(id: string) {
  meowApi.report(id).then(result => { run.value = result.run }).catch(e => emit('error', (e as Error).message))
}

async function start() {
  if (!canStart.value) return
  starting.value = true
  stopped.value = false
  try {
    const result = await meowApi.start({
      accountId: accountId.value,
      keyId: keyId.value,
      requestModel: requestModel.value,
      claimedModel: claimedModel.value,
      tier: tier.value,
    })
    run.value = result.run
    runs.value = (await meowApi.bootstrap()).runs
    emit('notice', 'Meow 检测已创建，正在逐题发送。')
    await pump()
  } catch (e) {
    emit('error', (e as Error).message)
  } finally {
    starting.value = false
  }
}

async function pump() {
  if (pumping.value || !run.value) return
  pumping.value = true
  stopped.value = false
  try {
    while (run.value && run.value.status === 'ready' && !stopped.value) {
      const result = await meowApi.step(run.value.id)
      run.value = result.run
      if (result.run.status === 'paused') {
        emit('notice', `第 ${result.run.attempts.length} 题失败，检测已暂停；继续检测会使用剩余重试预算。`)
      }
    }
    if (run.value && ['completed', 'stopped'].includes(run.value.status)) {
      runs.value = (await meowApi.bootstrap()).runs
      emit('notice', 'Meow 检测已结束，可在报告中查看行为指纹匹配。')
    }
  } catch (e) {
    stopped.value = true
    emit('error', (e as Error).message)
  } finally {
    pumping.value = false
  }
}

async function control(action: 'pause' | 'resume' | 'stop') {
  if (!run.value) return
  try {
    if (action === 'stop') stopped.value = true
    const result = await meowApi.control(run.value.id, action)
    run.value = result.run
    if (action === 'resume') await pump()
    if (action === 'pause') stopped.value = true
  } catch (e) {
    emit('error', (e as Error).message)
  }
}

async function remove(id: string) {
  try {
    await meowApi.delete(id)
    if (run.value?.id === id) run.value = undefined
    runs.value = (await meowApi.bootstrap()).runs
    emit('notice', '已删除该 Meow 检测记录。')
  } catch (e) {
    emit('error', (e as Error).message)
  }
}

onMounted(loadBootstrap)
</script>

<template>
  <section class="panel meow-panel">
    <div class="list-toolbar">
      <div class="list-filters">
        <span class="list-hint">Meow 行为指纹：按题档逐题发送，离线比对行为基线；不改账户、分组或调度。</span>
      </div>
      <div class="list-actions">
        <ZButton :disabled="loading || pumping" @click="loadBootstrap"><template #icon><RefreshCw :size="15" /></template>刷新</ZButton>
      </div>
    </div>

    <div class="meow-grid">
      <div class="meow-form">
        <div class="meow-field">
          <label>检测账号</label>
          <ZSelect v-model="accountId" :options="accounts.map(a => ({ value: a.id, label: a.email || a.name }))" placeholder="选择已启用的 OpenAI 账号" />
        </div>
        <div class="meow-field">
          <label>计量 Key</label>
          <ZSelect v-model="keyId" :options="keys.map(k => ({ value: k.id, label: k.name }))" placeholder="选择已有 Key" />
        </div>
        <div class="meow-field">
          <label>请求模型</label>
          <ZSelect v-model="requestModel" :options="models.map(m => ({ value: m, label: m }))" :disabled="!keyId" placeholder="先选择计量 Key" />
        </div>
        <div class="meow-field">
          <label>对照基线</label>
          <ZSelect v-model="claimedModel" :options="(catalog?.models ?? []).map(m => ({ value: m.id, label: m.label }))" placeholder="选择要核对的模型" />
        </div>
        <div class="meow-field">
          <label>题档</label>
          <ZSelect v-model="tier" :options="tierOptions" />
        </div>
        <div class="meow-start">
          <ZButton type="primary" :disabled="!canStart" :loading="starting" @click="start"><template #icon><Play :size="15" /></template>开始 Meow 检测（{{ count }} 题）</ZButton>
        </div>
        <p class="meow-note">检测会占用所选 Key 的额度，每题串行发送；失败自动暂停，可手动继续。</p>
      </div>

      <div class="meow-history">
        <h3>最近检测</h3>
        <p v-if="loading" class="cell-empty">正在读取…</p>
        <p v-else-if="!runs.length" class="cell-empty">还没有 Meow 检测记录。</p>
        <button v-for="item in runs" :key="item.id" type="button" class="meow-run" :class="{ active: run?.id === item.id }" @click="selectRun(item.id)">
          <span class="meow-run-main">
            <b>{{ item.accountName }}</b>
            <span class="pill dot" :class="`tone-${item.status === 'completed' ? 'success' : item.status === 'paused' ? 'warning' : item.status === 'stopped' ? 'neutral' : 'primary'}`">{{ statusText(item.status) }}</span>
          </span>
          <span class="meow-run-meta">{{ item.requestModel }} → {{ item.claimedModel }} · {{ item.tier }} · {{ item.attempts }}/{{ item.planned }} 题</span>
          <span class="meow-run-meta">{{ verdictText(item.verdict) }} · {{ date(item.updatedAt) }}</span>
        </button>
      </div>
    </div>

    <div v-if="run" class="meow-report">
      <div class="meow-report-head">
        <div>
          <h2>{{ run.accountName }}</h2>
          <p class="meow-run-meta">{{ run.plan?.request_model }} → {{ run.plan?.claimed_model }} · {{ run.plan?.tier }} · {{ run.keyName }}</p>
        </div>
        <div class="row-actions">
          <ZButton v-if="run.status === 'ready' && !pumping" size="small" :disabled="props.busy" @click="pump"><template #icon><Play :size="14" /></template>继续检测</ZButton>
          <ZButton v-if="pumping" size="small" @click="control('pause')"><template #icon><Square :size="12" /></template>当前题完成后暂停</ZButton>
          <ZButton v-if="!finished" size="small" :disabled="pumping" @click="control('stop')"><template #icon><Square :size="12" /></template>结束</ZButton>
          <ZButton v-if="finished" size="small" variant="text" @click="remove(run.id)"><template #icon><Trash2 :size="14" /></template>删除</ZButton>
        </div>
      </div>

      <div class="meow-progress">
        <span class="pill dot" :class="`tone-${run.status === 'completed' ? 'success' : run.status === 'paused' ? 'warning' : 'primary'}`">{{ statusText(run.status) }}</span>
        <span>有效样本 {{ okCount }} · 失败 {{ errCount }} · 计划 {{ planned }}</span>
        <span class="meter" aria-hidden="true"><i :style="{ width: `${progress}%` }" /></span>
        <b class="tabular">{{ progress }}%</b>
      </div>

      <div v-if="run.report?.reasons?.length" class="notice tone-warning">
        <TriangleAlert :size="15" aria-hidden="true" />
        <span>评分限制：{{ run.report.reasons.join('、') }}。样本未覆盖全部题目时结论只代表当前证据。</span>
      </div>

      <section class="meow-verdict" :class="verdictTone(run.report?.verdict)">
        <div class="meow-verdict-main">
          <span class="rpt-caption">行为指纹结论</span>
          <span class="pill dot lg" :class="verdictTone(run.report?.verdict)">{{ verdictText(run.report?.verdict) }}</span>
        </div>
        <dl class="rpt-facts">
          <div><dt>对照模型</dt><dd class="mono">{{ run.report?.claimed_model || run.plan?.claimed_model }}</dd></div>
          <div><dt>匹配模型</dt><dd class="mono">{{ run.report?.model || '—' }}</dd></div>
          <div><dt>引擎</dt><dd>{{ catalog?.engine_version || run.plan?.engine_version }} · {{ run.plan?.version }}</dd></div>
          <div><dt>基线摘要</dt><dd class="mono" :title="run.plan?.hash">{{ run.plan?.hash?.slice(0, 12) }}</dd></div>
        </dl>
      </section>

      <section v-if="topMatches.length" class="meow-matches">
        <h3>行为匹配度</h3>
        <div v-for="item in topMatches" :key="item.model" class="prob-row" :class="{ top: item.model === run.report?.model }">
          <span class="prob-name"><span class="mono">{{ item.model }}</span><span v-if="item.threshold != null" class="pill sm tone-neutral">阈值 {{ (item.threshold * 100).toFixed(1) }}%</span></span>
          <span class="prob-bar" aria-hidden="true"><i :style="{ width: `${Math.min(100, item.value * 100)}%` }" /><s v-if="item.threshold != null" :style="{ left: `${Math.min(100, item.threshold * 100)}%` }" /></span>
          <b class="prob-value tabular">{{ (item.value * 100).toFixed(1) }}%</b>
        </div>
      </section>

      <details class="rpt-fold">
        <summary><CircleCheck :size="15" aria-hidden="true" /><span>请求记录（{{ run.attempts.length }}）</span></summary>
        <ol class="attempt-list fold-body">
          <li v-for="(a, i) in run.attempts" :key="i" class="attempt" :class="{ failed: a.status === 'error' }">
            <span class="attempt-icon"><CircleAlert v-if="a.status === 'error'" :size="15" /><CircleCheck v-else :size="15" /></span>
            <div class="attempt-main">
              <div class="attempt-line"><span class="attempt-result">{{ a.error || `类别 ${a.category || '—'}` }}</span><time class="tabular">{{ a.request_id || '' }}</time></div>
              <div v-if="a.answer" class="attempt-meta"><code class="mono selectable">{{ a.answer }}</code></div>
            </div>
          </li>
        </ol>
      </details>

      <p class="meow-note"><ShieldAlert :size="13" aria-hidden="true" /> 行为指纹比较的是回答分布，不是智商分数或模型身份认证；库外模型也会归入最接近的候选。</p>
    </div>
  </section>
</template>

<style scoped>
.meow-panel { display: flex; flex-direction: column; gap: 18px; }
.meow-grid { display: grid; grid-template-columns: minmax(0, 1.2fr) minmax(0, 1fr); gap: 20px; }
.meow-form { display: flex; flex-direction: column; gap: 12px; }
.meow-field { display: flex; flex-direction: column; gap: 6px; }
.meow-field > label { font-size: 12px; font-weight: 600; color: var(--cp-text-muted, #667085); }
.meow-start { margin-top: 4px; }
.meow-note { display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--cp-text-muted, #667085); margin: 0; }
.meow-history h3, .meow-matches h3 { font-size: 13px; margin: 0 0 8px; }
.meow-run { display: flex; flex-direction: column; gap: 4px; width: 100%; text-align: left; padding: 10px 12px; margin-bottom: 8px; border: 1px solid var(--cp-border, #e4e7ec); border-radius: 10px; background: transparent; cursor: pointer; }
.meow-run:hover { border-color: var(--cp-primary, #2563eb); }
.meow-run.active { border-color: var(--cp-primary, #2563eb); background: var(--cp-primary-soft, rgba(37, 99, 235, 0.06)); }
.meow-run-main { display: flex; align-items: center; gap: 8px; }
.meow-run-meta { font-size: 12px; color: var(--cp-text-muted, #667085); }
.meow-report { display: flex; flex-direction: column; gap: 16px; border-top: 1px solid var(--cp-border, #e4e7ec); padding-top: 16px; }
.meow-report-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; }
.meow-report-head h2 { margin: 0; font-size: 16px; }
.meow-progress { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; font-size: 12px; color: var(--cp-text-muted, #667085); }
.meow-progress .meter { flex: 1; min-width: 120px; height: 6px; border-radius: 999px; background: var(--cp-border, #e4e7ec); overflow: hidden; }
.meow-progress .meter i { display: block; height: 100%; background: var(--cp-primary, #2563eb); }
.meow-verdict { display: flex; flex-wrap: wrap; align-items: center; gap: 20px; padding: 14px 16px; border-radius: 12px; border: 1px solid var(--cp-border, #e4e7ec); }
.meow-verdict-main { display: flex; flex-direction: column; gap: 6px; }
.meow-matches { display: flex; flex-direction: column; gap: 6px; }
@media (max-width: 900px) { .meow-grid { grid-template-columns: 1fr; } }
</style>
