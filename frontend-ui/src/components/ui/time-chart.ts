import { LitElement, html } from 'lit';
import { customElement, property } from 'lit/decorators.js';

export interface TimeSeries {
  name: string;
  data: Array<[string | number, number]>;
  color?: string;
  type?: 'line' | 'bar';
  stack?: string;
  area?: boolean;
}

export type YFormat = 'number' | 'usd' | 'count';

function readVar(el: HTMLElement, name: string): string {
  return getComputedStyle(el).getPropertyValue(name).trim();
}

type EChartsModule = typeof import('echarts');
type EChartsInstance = import('echarts').ECharts;

let echartsModulePromise: Promise<EChartsModule> | null = null;

function loadEcharts(): Promise<EChartsModule> {
  if (!echartsModulePromise) {
    echartsModulePromise = import('echarts');
  }
  return echartsModulePromise;
}

@customElement('time-chart')
export class TimeChart extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  @property({ attribute: false }) series: TimeSeries[] = [];
  @property() yFormat: YFormat = 'number';
  @property() chartHeading = '';

  private chart: EChartsInstance | null = null;
  private resizeObserver: ResizeObserver | null = null;
  private themeObserver: MutationObserver | null = null;
  // Cached signature so we only call setOption when the series payload actually
  // changes — Lit's `updated()` fires on every parent re-render even when the
  // series array has the same shape, and re-applying ECharts options during a
  // user's hover wipes the hover state and makes lines flicker / disappear.
  private _lastSig = '';

  override firstUpdated(): void {
    void this._initChart();
  }

  private async _initChart(): Promise<void> {
    const el = this.renderRoot.querySelector('.chart') as HTMLElement | null;
    if (!el) return;
    const echarts = await loadEcharts();
    if (!this.isConnected) return;
    this.chart = echarts.init(el, null, { renderer: 'canvas' });
    this._render(true);
    this.resizeObserver = new ResizeObserver(() => this.chart?.resize());
    this.resizeObserver.observe(el);
    this.themeObserver = new MutationObserver(() => this._render(true));
    this.themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['data-theme'],
    });
  }

  override updated(): void {
    this._render(false);
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.resizeObserver?.disconnect();
    this.themeObserver?.disconnect();
    this.chart?.dispose();
    this.chart = null;
  }

  private _formatY(): (n: number) => string {
    if (this.yFormat === 'usd') {
      return (n) =>
        new Intl.NumberFormat(undefined, {
          style: 'currency',
          currency: 'USD',
          notation: 'compact',
          maximumFractionDigits: 1,
        }).format(n);
    }
    if (this.yFormat === 'count') {
      return (n) =>
        new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 }).format(n);
    }
    return (n) =>
      new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 2 }).format(n);
  }

  private _render(force: boolean): void {
    if (!this.chart) return;
    // Skip when neither the data nor anything we read from CSS has changed.
    const sig = JSON.stringify({
      yf: this.yFormat,
      theme: document.documentElement.getAttribute('data-theme'),
      series: this.series.map((s) => ({ n: s.name, t: s.type, st: s.stack, a: s.area, c: s.color, d: s.data })),
    });
    if (!force && sig === this._lastSig) return;
    this._lastSig = sig;

    const root = document.documentElement;
    const fg = readVar(root, '--fg') || '#111';
    const muted = readVar(root, '--fg-muted') || '#666';
    const grid = readVar(root, '--border') || '#ddd';
    const accent = readVar(root, '--accent') || '#2563eb';
    const pos = readVar(root, '--pos') || '#166534';
    const palette = [accent, pos, muted, '#a16207', '#9333ea'];

    this.chart.setOption(
      {
        color: palette,
        textStyle: { color: fg, fontFamily: readVar(root, '--font-sans') },
        grid: { left: 56, right: 16, top: 36, bottom: 40 },
        legend: {
          top: 0,
          left: 0,
          textStyle: { color: muted },
          icon: 'roundRect',
          itemHeight: 8,
        },
        tooltip: {
          trigger: 'axis',
          backgroundColor: readVar(root, '--bg-elev'),
          borderColor: grid,
          textStyle: { color: fg },
          axisPointer: { type: 'line', lineStyle: { color: grid, width: 1 } },
        },
        xAxis: {
          type: 'time',
          axisLine: { lineStyle: { color: grid } },
          axisLabel: { color: muted },
          splitLine: { show: false },
        },
        yAxis: {
          type: 'value',
          axisLine: { show: false },
          axisLabel: { color: muted, formatter: this._formatY() },
          splitLine: { lineStyle: { color: grid, type: 'dashed' } },
        },
        series: this.series.map((s, i) => ({
          name: s.name,
          type: s.type ?? 'line',
          stack: s.stack,
          smooth: true,
          showSymbol: false,
          lineStyle: { width: 2 },
          itemStyle: s.color ? { color: s.color } : undefined,
          areaStyle: s.area
            ? { color: s.color ?? palette[i % palette.length], opacity: 0.15 }
            : undefined,
          // Keep all series fully visible on hover. ECharts' default
          // emphasis can fade non-hovered series — combined with the 0.15
          // areaStyle opacity that produced effectively-invisible fills.
          emphasis: { focus: 'none', scale: false },
          blur: { lineStyle: { opacity: 1 }, areaStyle: { opacity: 0.15 } },
          data: s.data,
        })),
        backgroundColor: 'transparent',
      },
      // Don't blow away the chart on every update — that resets hover state
      // and is the actual cause of "lines disappear when hovering". Only
      // notMerge on initial init / theme change / data change.
      { notMerge: true, lazyUpdate: true },
    );
  }

  override render() {
    return html`<div class="chart" role="img" aria-label="${this.chartHeading || 'Time-series chart'}"></div>`;
  }
}
