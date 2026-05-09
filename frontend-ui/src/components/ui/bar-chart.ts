import { LitElement, html } from 'lit';
import { customElement, property } from 'lit/decorators.js';

export interface BarDatum {
  label: string;
  value: number;
  color?: string;
}

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

@customElement('bar-chart')
export class BarChart extends LitElement {
  override createRenderRoot(): HTMLElement { return this; }


  @property({ attribute: false }) data: BarDatum[] = [];
  @property() horizontal = false;
  @property() yFormat: 'number' | 'usd' | 'count' = 'number';

  private chart: EChartsInstance | null = null;
  private resizeObserver: ResizeObserver | null = null;
  private themeObserver: MutationObserver | null = null;

  override firstUpdated(): void {
    void this._initChart();
  }

  private async _initChart(): Promise<void> {
    const el = this.renderRoot.querySelector('.chart') as HTMLElement | null;
    if (!el) return;
    const echarts = await loadEcharts();
    if (!this.isConnected) return;
    this.chart = echarts.init(el, null, { renderer: 'canvas' });
    this._render();
    this.resizeObserver = new ResizeObserver(() => this.chart?.resize());
    this.resizeObserver.observe(el);
    this.themeObserver = new MutationObserver(() => this._render());
    this.themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['data-theme'],
    });
  }

  override updated(): void { this._render(); }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    this.resizeObserver?.disconnect();
    this.themeObserver?.disconnect();
    this.chart?.dispose();
    this.chart = null;
  }

  private _formatV(): (n: number) => string {
    if (this.yFormat === 'usd') {
      return (n) =>
        new Intl.NumberFormat(undefined, {
          style: 'currency',
          currency: 'USD',
          notation: 'compact',
          maximumFractionDigits: 1,
        }).format(n);
    }
    return (n) =>
      new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 }).format(n);
  }

  private _render(): void {
    if (!this.chart) return;
    const root = document.documentElement;
    const fg = readVar(root, '--fg') || '#111';
    const muted = readVar(root, '--fg-muted') || '#666';
    const grid = readVar(root, '--border') || '#ddd';
    const accent = readVar(root, '--accent') || '#2563eb';

    const labels = this.data.map((d) => d.label);
    const values = this.data.map((d) => ({ value: d.value, itemStyle: d.color ? { color: d.color } : undefined }));

    const categoryAxis = {
      type: 'category' as const,
      data: labels,
      axisLine: { lineStyle: { color: grid } },
      axisLabel: { color: muted, interval: 0, hideOverlap: true },
    };
    const valueAxis = {
      type: 'value' as const,
      axisLine: { show: false },
      axisLabel: { color: muted, formatter: this._formatV() },
      splitLine: { lineStyle: { color: grid, type: 'dashed' as const } },
    };

    this.chart.setOption(
      {
        color: [accent],
        textStyle: { color: fg, fontFamily: readVar(root, '--font-sans') },
        grid: { left: 80, right: 16, top: 16, bottom: 40 },
        tooltip: {
          trigger: 'axis',
          axisPointer: { type: 'shadow' },
          backgroundColor: readVar(root, '--bg-elev'),
          borderColor: grid,
          textStyle: { color: fg },
        },
        xAxis: this.horizontal ? valueAxis : categoryAxis,
        yAxis: this.horizontal ? categoryAxis : valueAxis,
        series: [
          {
            type: 'bar',
            data: values,
            barMaxWidth: 32,
          },
        ],
        backgroundColor: 'transparent',
      },
      { notMerge: true },
    );
  }

  override render() {
    return html`<div class="chart" role="img" aria-label="Bar chart"></div>`;
  }
}
