// Cesium 地球:点云渲染、两帧之间插值、拾取、轨道线。

/** 动态加载 CesiumJS(基础 URL 由服务端 /api/v1/client-config 提供)。 */
export function loadCesium(baseUrl, integrity) {
  window.CESIUM_BASE_URL = baseUrl;
  const css = document.createElement('link');
  css.rel = 'stylesheet';
  // 从 CDN 加载时校验内容(SRI):文件被篡改或替换时浏览器拒绝使用
  if (integrity) {
    css.integrity = integrity.css;
    css.crossOrigin = 'anonymous';
  }
  css.href = `${baseUrl}Widgets/widgets.css`;
  document.head.appendChild(css);
  return new Promise((resolve, reject) => {
    const s = document.createElement('script');
    if (integrity) {
      s.integrity = integrity.js;
      s.crossOrigin = 'anonymous';
    }
    s.src = `${baseUrl}Cesium.js`;
    s.onload = () => (window.Cesium ? resolve(window.Cesium) : reject(new Error('Cesium 未定义')));
    s.onerror = () => reject(Object.assign(new Error(`CesiumJS load failed: ${baseUrl}`), { cesiumUrl: baseUrl }));
    document.head.appendChild(s);
  });
}

function cssColor(Cesium, varName) {
  const v = getComputedStyle(document.documentElement).getPropertyValue(varName).trim();
  return Cesium.Color.fromCssColorString(v || '#999999');
}

export class Globe {
  constructor(Cesium, container) {
    this.C = Cesium;
    const viewer = new Cesium.Viewer(container, {
      // 使用 Cesium 自带的 NaturalEarthII 离线影像,不依赖 Cesium ion
      baseLayer: Cesium.ImageryLayer.fromProviderAsync(
        Cesium.TileMapServiceImageryProvider.fromUrl(Cesium.buildModuleUrl('Assets/Textures/NaturalEarthII')),
      ),
      baseLayerPicker: false,
      geocoder: false,
      timeline: false,
      animation: false,
      homeButton: true,
      sceneModePicker: true,
      navigationHelpButton: false,
      fullscreenButton: false,
      infoBox: false,
      selectionIndicator: false,
      shouldAnimate: false,
    });
    this.viewer = viewer;
    viewer.scene.globe.enableLighting = true;
    viewer.scene.fog.enabled = false;
    viewer.camera.setView({ destination: Cesium.Cartesian3.fromDegrees(110, 20, 28_000_000) });

    this.colors = {
      PAYLOAD: cssColor(Cesium, '--type-payload'),
      'ROCKET BODY': cssColor(Cesium, '--type-rocket'),
      DEBRIS: cssColor(Cesium, '--type-debris'),
      UNKNOWN: cssColor(Cesium, '--type-unknown'),
    };
    this.points = viewer.scene.primitives.add(new Cesium.PointPrimitiveCollection());
    this.nearFar = new Cesium.NearFarScalar(1.5e6, 2.0, 4.0e7, 1.0);
    this.bySlot = []; // 槽位 → PointPrimitive
    this.slotOf = new Map(); // 编目下标 → 槽位
    this.objects = new Map(); // 编目下标 → 元数据
    this.prev = new Float64Array(0);
    this.cur = new Float64Array(0);
    this.visible = new Uint8Array(0);
    this.frameWall = 0; // 收到最新帧时的 performance.now()
    this.frameDt = 1000;
    this.frameTimeMs = 0;
    this.prevTimeMs = 0;
    this.selected = null;
    this.trackEntities = [];
    this.scratch = new Cesium.Cartesian3();

    viewer.scene.preRender.addEventListener(() => this.interpolate());
  }

  /** 设置当前订阅的物体集合(收到 catalog 消息时调用)。 */
  setCatalog(objects) {
    const C = this.C;
    this.points.removeAll();
    this.bySlot = [];
    this.slotOf.clear();
    this.objects.clear();
    const n = objects.length;
    this.prev = new Float64Array(n * 3);
    this.cur = new Float64Array(n * 3);
    this.visible = new Uint8Array(n);
    objects.forEach((o, slot) => {
      this.slotOf.set(o.i, slot);
      this.objects.set(o.i, o);
      const p = this.points.add({
        position: C.Cartesian3.ZERO,
        color: this.colors[o.type] || this.colors.UNKNOWN,
        pixelSize: 3,
        // 近处放大(最多 2 倍),远处保持 3 像素,避免全球视图过于拥挤
        scaleByDistance: this.nearFar,
        show: false,
        id: o.i,
      });
      this.bySlot.push(p);
    });
    if (this.selected !== null && this.slotOf.has(this.selected)) this.highlight(this.selected, true);
  }

  /** 应用一帧位置。点在两帧之间线性插值(显示滞后一帧,换取平滑)。 */
  applyFrame(f) {
    const now = performance.now();
    if (this.frameWall) this.frameDt = Math.min(Math.max(now - this.frameWall, 100), 5000);
    this.frameWall = now;
    this.prevTimeMs = this.frameTimeMs || f.timeMs;
    this.frameTimeMs = f.timeMs;

    // 旧的目标位置成为新的起点
    const seen = new Uint8Array(this.bySlot.length);
    for (let k = 0; k < f.count; k++) {
      const slot = this.slotOf.get(f.idx[k]);
      if (slot === undefined) continue;
      const o = slot * 3;
      const x = f.xyz[3 * k] * 1000, y = f.xyz[3 * k + 1] * 1000, z = f.xyz[3 * k + 2] * 1000;
      if (this.visible[slot]) {
        this.prev[o] = this.cur[o]; this.prev[o + 1] = this.cur[o + 1]; this.prev[o + 2] = this.cur[o + 2];
      } else {
        this.prev[o] = x; this.prev[o + 1] = y; this.prev[o + 2] = z;
      }
      this.cur[o] = x; this.cur[o + 1] = y; this.cur[o + 2] = z;
      seen[slot] = 1;
    }
    for (let s = 0; s < this.bySlot.length; s++) {
      if (seen[s] !== this.visible[s]) this.bySlot[s].show = !!seen[s];
    }
    this.visible = seen;
    return f.count;
  }

  /** 每次渲染前调用:插值并同步场景时钟(用于日照)。 */
  interpolate() {
    if (!this.frameWall) return;
    const a = Math.min((performance.now() - this.frameWall) / this.frameDt, 1);
    const p = this.scratch;
    for (let s = 0; s < this.bySlot.length; s++) {
      if (!this.visible[s]) continue;
      const o = s * 3;
      p.x = this.prev[o] + (this.cur[o] - this.prev[o]) * a;
      p.y = this.prev[o + 1] + (this.cur[o + 1] - this.prev[o + 1]) * a;
      p.z = this.prev[o + 2] + (this.cur[o + 2] - this.prev[o + 2]) * a;
      this.bySlot[s].position = p;
    }
    const t = this.prevTimeMs + (this.frameTimeMs - this.prevTimeMs) * a;
    this.viewer.clock.currentTime = this.C.JulianDate.fromDate(new Date(t));
  }

  /** 当前显示时刻(毫秒)。 */
  displayTimeMs() {
    if (!this.frameWall) return null;
    const a = Math.min((performance.now() - this.frameWall) / this.frameDt, 1);
    return this.prevTimeMs + (this.frameTimeMs - this.prevTimeMs) * a;
  }

  /** 某物体当前显示位置(Cartesian3,米);不在显示集合中时为 null。 */
  positionOf(i) {
    const slot = this.slotOf.get(i);
    if (slot === undefined || !this.visible[slot]) return null;
    return this.C.Cartesian3.clone(this.bySlot[slot].position);
  }

  pick(windowPos) {
    const picked = this.viewer.scene.pick(windowPos);
    if (picked && picked.primitive && picked.collection === this.points) return picked.id;
    return null;
  }

  highlight(i, on) {
    const slot = this.slotOf.get(i);
    if (slot === undefined) return;
    const p = this.bySlot[slot];
    p.pixelSize = on ? 8 : 3;
    p.outlineColor = this.C.Color.WHITE;
    p.outlineWidth = on ? 2 : 0;
  }

  select(i) {
    if (this.selected !== null) this.highlight(this.selected, false);
    this.selected = i;
    if (i !== null) this.highlight(i, true);
    if (i === null) this.clearTrack();
  }

  clearTrack() {
    for (const e of this.trackEntities) this.viewer.entities.remove(e);
    this.trackEntities = [];
  }

  /** 绘制轨迹:地固系中的空间轨迹(实线)与星下点轨迹(虚线)。 */
  showTrack(points, typeKey) {
    const C = this.C;
    this.clearTrack();
    if (!points.length) return;
    const color = this.colors[typeKey] || this.colors.UNKNOWN;
    const path = points.map((p) => new C.Cartesian3(p.ecef_pos_km[0] * 1000, p.ecef_pos_km[1] * 1000, p.ecef_pos_km[2] * 1000));
    const ground = points.map((p) => C.Cartesian3.fromDegrees(p.geodetic.lon_deg, p.geodetic.lat_deg, 0));
    this.trackEntities.push(
      this.viewer.entities.add({ polyline: { positions: path, width: 2, material: color.withAlpha(0.9), arcType: C.ArcType.NONE } }),
      this.viewer.entities.add({
        polyline: {
          positions: ground,
          width: 1.5,
          material: new C.PolylineDashMaterialProperty({ color: color.withAlpha(0.7), dashLength: 12 }),
          clampToGround: false,
          arcType: C.ArcType.GEODESIC,
        },
      }),
    );
  }

  flyTo(i) {
    const pos = this.positionOf(i);
    if (!pos) return;
    const C = this.C;
    const carto = C.Cartographic.fromCartesian(pos);
    this.viewer.camera.flyTo({
      destination: C.Cartesian3.fromRadians(carto.longitude, carto.latitude, carto.height + 4_000_000),
      duration: 1.5,
    });
  }
}
