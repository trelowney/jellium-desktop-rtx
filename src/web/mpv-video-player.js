(function() {
    function getMediaStreamAudioTracks(mediaSource) {
        return mediaSource.MediaStreams.filter(s => s.Type === 'Audio');
    }

    // Convert Jellyfin global MediaStream.Index to 1-based type-relative index
    function getRelativeIndexByType(mediaStreams, jellyIndex, streamType) {
        let relIndex = 1;
        for (const source of mediaStreams) {
            if (source.Type !== streamType || source.IsExternal) continue;
            if (source.Index === jellyIndex) return relIndex;
            relIndex += 1;
        }
        return null;
    }

    function getStreamByIndex(mediaStreams, index) {
        return mediaStreams.find(s => s.Index === index) || null;
    }

    // ---- Playback Info: forward-buffer formatting -----------------------
    // Fed by window._nativeBufferStats (mpv's demuxer-cache-state, ~1 Hz).
    const BUFFER_STATS_STALE_MS = 5000;
    const NOT_AVAILABLE = '—';  // em dash

    function formatBufferBytes(bytes) {
        const mb = bytes / (1024 * 1024);
        return mb >= 1000 ? `${(mb / 1024).toFixed(2)} GB` : `${mb.toFixed(mb < 10 ? 1 : 0)} MB`;
    }

    function formatBufferRate(bytesPerSec) {
        const mb = bytesPerSec / (1024 * 1024);
        return mb >= 1 ? `${mb.toFixed(1)} MB/s` : `${Math.round(bytesPerSec / 1024)} kB/s`;
    }

    function formatBufferClock(seconds) {
        const total = Math.round(seconds);
        const s = String(total % 60).padStart(2, '0');
        const m = Math.floor(total / 60) % 60;
        const h = Math.floor(total / 3600);
        return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
    }

    // What the driver will and will not tell us about RTX, and why these rows
    // are worded the way they are:
    //
    // The NVIDIA driver exposes no way to read back whether Super Resolution is
    // engaged. That was measured against the driver directly, not assumed: the
    // extension answers a query, but the answer is identical whether Super
    // Resolution is on or off in the NVIDIA Control Panel, and identical whether
    // the conversion upscales or not. So a live "is it applying right now" for
    // VSR is not obtainable, and claiming one would be dishonest.
    //
    // What IS observable is what the filter chain did to the frames, via mpv's
    // video-params (decoded), video-out-params (after d3d11vpp) and
    // video-target-params (sent to the display). RTX Video HDR is genuinely
    // verifiable this way: the conversion has to move the output to PQ/BT.2020,
    // so if it did not, it did not happen. Scaling is verifiable too, though it
    // only proves d3d11vpp scaled — not that NVIDIA's AI path did it rather than
    // the filter's fallback scaler. The wording keeps that distinction.
    function pipelineStages() {
        const p = window.__videoPipeline;
        if (!p) return null;
        const known = (s) => s && s.w > 0 && s.h > 0;
        return known(p.source) ? p : null;
    }

    function sizeOf(stage) {
        return stage && stage.w > 0 && stage.h > 0 ? `${stage.w}×${stage.h}` : null;
    }

    // mpv reports PQ as "pq"; BT.2020 primaries as "bt.2020". Either alone is
    // not HDR, but the RTX conversion sets both.
    function isHdr(stage) {
        return !!stage && stage.gamma === 'pq';
    }

    function rtxVsrStatus(enabled, runtime) {
        if (!enabled) return 'Off';
        if (runtime === 'failed') return 'Failed (GPU rejected)';
        if (runtime === 'unsupported') return 'Unsupported (no NVIDIA GPU)';

        const p = pipelineStages();
        if (!p) return runtime === 'active' ? 'Enabled (driver accepted)' : 'Enabled';

        const src = p.source;
        const out = p.filtered;
        if (!(out && out.w > 0)) return 'Enabled (driver accepted)';
        if (out.w <= src.w) {
            // Not a fault: the scaling stage is dropped when the output is no
            // larger than the source, since anything upscaled would be resized
            // straight back down again.
            return 'Enabled — not upscaling (output is not larger than the source)';
        }
        const factor = (out.w / src.w).toFixed(2).replace(/\.?0+$/, '');
        let value = `Scaling ${sizeOf(src)} → ${sizeOf(out)} (${factor}×)`;
        // The filter scales by a fixed factor, so on a display smaller than its
        // output the video output resamples once more. That is expected, not a
        // fault, so it is stated plainly rather than flagged.
        const target = p.target;
        if (target && target.w > 0 && target.w !== out.w) {
            value += `, display ${sizeOf(target)}`;
        }
        return value;
    }

    function rtxHdrStatus(enabled, runtime) {
        if (!enabled) return 'Off';
        if (runtime === 'failed') return 'Failed (GPU rejected)';
        if (runtime === 'unsupported') return 'Unsupported (driver reported no support)';

        const p = pipelineStages();
        if (!p) return 'Enabled';

        // A source that is already HDR has nothing to convert.
        if (isHdr(p.source)) return 'Not needed (source is already HDR)';

        // The conversion is only real if it shows up on the filter output.
        if (isHdr(p.filtered)) {
            const target = p.target;
            const onDisplay = isHdr(target) ? '' : ', but the display is not in HDR';
            return `Active — output converted to PQ / ${p.filtered.primaries || 'BT.2020'}${onDisplay}`;
        }
        if (p.filtered && p.filtered.gamma) {
            return `Enabled, but output is still ${p.filtered.gamma} (not converting)`;
        }
        return 'Enabled';
    }

    // Since the driver will not report whether Super Resolution is engaged, its
    // cost is the closest available evidence: a GPU near idle during an upscale
    // is not upscaling. Stale samples are dropped rather than shown as current.
    function describeGpuLoad() {
        const l = window.__gpuLoad;
        if (!l || typeof l.gpu !== 'number') return null;
        if (Date.now() - l.at > 5000) return null;
        return `${l.gpu}% (memory ${l.memory}%)`;
    }

    // The raw evidence behind the two rows above.
    function describePipeline() {
        const p = pipelineStages();
        if (!p) return null;
        const stage = (s) => {
            const size = sizeOf(s);
            if (!size) return null;
            return s.gamma ? `${size} ${s.gamma}` : size;
        };
        const parts = [stage(p.source), stage(p.filtered), stage(p.target)].filter(Boolean);
        return parts.length > 1 ? parts.join(' → ') : null;
    }

    // Reads as a sentence in four rows: how much is buffered, how much playback
    // time that covers, how fast it is filling, and what the demuxer is doing.
    function getBufferStatsCategory() {
        const b = window.__bufferStats;
        if (!b || typeof b.fwBytes !== 'number') return null;
        const stale = Date.now() - b.at > BUFFER_STATS_STALE_MS;

        let buffered = formatBufferBytes(b.fwBytes);
        if (b.maxBytes > 0) {
            const percent = Math.min(100, Math.round((b.fwBytes / b.maxBytes) * 100));
            buffered += ` of ${formatBufferBytes(b.maxBytes)} (${percent}%)`;
        }

        // The rate is only meaningful while mpv is still reading — a paused or
        // fully-buffered stream stops updating, so show a dash instead of the
        // last number it happened to report.
        const rate = stale || typeof b.rateBps !== 'number' ? NOT_AVAILABLE : formatBufferRate(b.rateBps);
        const ahead = typeof b.seconds === 'number' ? formatBufferClock(b.seconds) : NOT_AVAILABLE;

        let status;
        if (b.underrun) status = 'Underrun (waiting for data)';
        else if (b.eofCached) status = 'End of stream buffered';
        else if (stale || b.idle) status = b.maxBytes > 0 && b.fwBytes >= b.maxBytes * 0.95 ? 'Full' : 'Idle';
        else status = 'Filling';

        return {
            name: 'Playback Buffer',
            stats: [
                { label: 'Buffered ahead', value: buffered },
                { label: 'Playback time buffered', value: ahead },
                { label: 'Fill rate', value: rate },
                { label: 'Status', value: status }
            ]
        };
    }

    // ---- Clear logo under the OSD title -----------------------------------
    // jellyfin-web titles the video OSD with plain text
    // ("Neagley - S1:E6 - Rocked (2026)"). The server usually has the series'
    // or movie's clear logo too, so hang that under the title line, the way
    // Stremio does. The text is left untouched: in jellyfin-web 12 it is React
    // state inside VideoPage's toolbar, and a sibling inserted after the
    // toolbar is something React never reconciles. The header (.osdHeader) is
    // 7.5em tall with a top gradient and fades with the OSD, so the logo needs
    // no show/hide plumbing of its own; the node dies with the page.
    const OSD_LOGO_CLASS = 'rtxOsdLogo';
    const OSD_LOGO_HEIGHT_PX = 240;  // requested from the server; CSS scales it down

    // Episodes carry their series' logo as ParentLogo*; movies (and series
    // played directly) carry it in their own ImageTags.
    function osdLogoSource(item) {
        if (!item) return null;
        if (item.ParentLogoItemId && item.ParentLogoImageTag) {
            return { id: item.ParentLogoItemId, tag: item.ParentLogoImageTag };
        }
        const tag = item.ImageTags?.Logo;
        return tag ? { id: item.Id, tag } : null;
    }

    function osdLogoUrl(src, mediaUrl) {
        let base = window.ApiClient?.serverAddress?.();
        if (!base && mediaUrl) {
            try { base = new URL(mediaUrl, location.href).origin; } catch { base = null; }
        }
        if (!base) return null;
        const q = new URLSearchParams({ tag: src.tag, maxHeight: String(OSD_LOGO_HEIGHT_PX), quality: '90' });
        return `${base.replace(/\/$/, '')}/Items/${src.id}/Images/Logo?${q}`;
    }

    function removeOsdLogo() {
        for (const el of document.querySelectorAll('.' + OSD_LOGO_CLASS)) el.remove();
    }

    // `.videoOsd-appBar` is the React toolbar (jellyfin-web 12), `.headerTop`
    // the legacy skin header. The OSD controller also tags the (hidden) legacy
    // skin header with .osdHeader on the React layout, so pick the one that is
    // actually laid out. Retries across frames: the title event can land while
    // the OSD page is still mounting.
    function findOsdHeader() {
        for (const header of document.querySelectorAll('.osdHeader')) {
            const toolbar = header.querySelector('.videoOsd-appBar, .headerTop');
            if (toolbar && header.offsetWidth > 0) return { header, toolbar };
        }
        return null;
    }

    function placeOsdLogo(url, attempt = 0) {
        const found = findOsdHeader();
        if (!found) {
            if (attempt < 60) requestAnimationFrame(() => placeOsdLogo(url, attempt + 1));
            return;
        }
        const { header, toolbar } = found;
        const existing = header.querySelector('.' + OSD_LOGO_CLASS);
        if (existing?.dataset.url === url) return;
        removeOsdLogo();

        // Line the logo up with the title text, not with the back arrow.
        const title = toolbar.querySelector('.MuiTypography-root, .pageTitle');
        const left = title
            ? Math.max(0, Math.round(title.getBoundingClientRect().left - header.getBoundingClientRect().left))
            : 0;

        const box = document.createElement('div');
        box.className = OSD_LOGO_CLASS;
        box.dataset.url = url;
        box.style.cssText = `padding:0.35em 1em 0 ${left}px;pointer-events:none;`;
        const img = document.createElement('img');
        img.alt = '';
        img.draggable = false;
        img.style.cssText = 'display:block;height:3.4em;max-width:26em;object-fit:contain;object-position:left center;filter:drop-shadow(0 2px 4px rgba(0,0,0,.7));';
        img.addEventListener('error', () => box.remove());
        img.src = url;
        box.appendChild(img);
        toolbar.insertAdjacentElement('afterend', box);
    }

    class mpvVideoPlayer extends window.MpvPlayerBase {
        constructor(args) {
            super(args);
            const { loading, appRouter, globalize, dashboard, playbackManager } = args;
            this.loading = loading;
            this.appRouter = appRouter;
            this.globalize = globalize;
            this.playbackManager = playbackManager;
            if (dashboard && dashboard.default) {
                this.setTransparency = dashboard.default.setBackdropTransparency.bind(dashboard);
            } else {
                this.setTransparency = () => {};
            }

            this.id = 'mpvvideoplayer';
            this.logTag = 'Video';
            this.name = 'MPV Video Player';
            this.syncPlayWrapAs = 'htmlvideoplayer';
            this.priority = -1;
            this.useFullSubtitleUrls = true;
            this.isLocalPlayer = true;
            this.isFetching = false;

            window._mpvVideoPlayerInstance = this;

            // jellyfin-web's Events bus keeps listeners in obj._callbacks and
            // the OSD announces its title on `document`; register the same way
            // Events.on would, since the module itself is not reachable here.
            const bus = (document._callbacks = document._callbacks || {});
            (bus.VIDEO_TITLE_CHANGE = bus.VIDEO_TITLE_CHANGE || []).push((_e, title) => this.onOsdTitleChange(title));

            this._videoDialog = undefined;
            this._currentSrc = undefined;
            this._timeUpdated = false;
            this._currentPlayOptions = undefined;
            this._endedPending = false;

            // Support jellyfin-web v10.10.7
            this._currentAspectRatio = undefined;

            this.handlers.onPlaying = () => {
                if (!this._started) {
                    this._started = true;
                    this.loading.hide();
                    const dlg = this._videoDialog;
                    // Remove poster so video shows through from subsurface
                    if (dlg) {
                        const poster = dlg.querySelector('.mpvPoster');
                        if (poster) poster.remove();
                    }
                    // "fullscreen" = fills entire web content area, not the actual screen
                    if (this._currentPlayOptions?.fullscreen) {
                        this.appRouter.showVideoOsd();
                        if (dlg) dlg.style.zIndex = 'unset';
                    }
                    window.api.player.setVideoRectangle(0, 0, 0, 0);
                }
                this._emitPlaying();
            };

            this.handlers.onTimeUpdate = (time) => {
                if (time && !this._timeUpdated) this._timeUpdated = true;
                this._seeking = false;
                this._currentTime = time;
                this.events.trigger(this, 'timeupdate');
            };

            this.handlers.onEnded = () => {
                if (!this._endedPending) {
                    this._endedPending = true;
                    this.onEndedInternal();
                }
            };

            this.handlers.onError = (error) => {
                this.removeMediaDialog();
                console.error(`[Media] [${this.logTag}] media error:`, error);
                this.events.trigger(this, 'error', [{ type: 'mediadecodeerror' }]);
            };
        }

        async play(options) {
            console.debug(`[Media] [${this.logTag}] play() called with options:`, options);
            this._started = false;
            this._timeUpdated = false;
            this._currentTime = null;
            this._endedPending = false;
            if (options.resetSubtitleOffset !== false) this.resetSubtitleOffset();
            if (options.fullscreen) this.loading.show();  // fills entire web content area, not the actual screen
            await this.createMediaElement(options);
            console.debug(`[Media] [${this.logTag}] createMediaElement done, calling setCurrentSrc`);
            const result = await this.setCurrentSrc(options);

            // needed when only audio is single external
            const externalAudio = options.mediaSource?.MediaStreams?.find(s => s.Type === 'Audio' && s.IsExternal);
            if (externalAudio && options.playMethod !== 'Transcode') {
                this.setAudioStreamIndex(externalAudio.Index);
            }
            return result;
        }

        get mediaType() { return 'video'; }

        _resolveTracks(options) {
            const streams = options.mediaSource?.MediaStreams || [];
            let defaultAudioIdx = options.mediaSource.DefaultAudioStreamIndex ?? -1;
            const defaultSubIdx = options.mediaSource.DefaultSubtitleStreamIndex ?? -1;

            if (defaultAudioIdx < 0) {
                const fallback = streams.find(s => s.Type === 'Audio' && !s.IsExternal)
                    ?? streams.find(s => s.Type === 'Audio');
                if (fallback) defaultAudioIdx = fallback.Index;
            }

            // Mirror jellyfin-web's UI selection exactly: feed mpv the relative
            // index for DefaultAudioStreamIndex, or TRACK_DISABLE if none is selected.
            // mpv auto track selection is completely disabled as it conflicts with
            // the fact that jellyfin-web is ultimately responsible for that.
            let audioParam = MpvPlayerBase.TRACK_DISABLE;
            let externalAudioUrl = null;
            if (options.playMethod === 'Transcode') {
                // Server bakes the chosen audio into the transcoded output
                // (single audio track in the m3u8). Source MediaStreams indexing
                // doesn't apply — see htmlVideoPlayer/plugin.js:514 for the same
                // logic. Don't audio-add either; audio is already in the stream.
                audioParam = 1;
            } else if (defaultAudioIdx >= 0) {
                const audioStream = getStreamByIndex(streams, defaultAudioIdx);
                if (audioStream && audioStream.DeliveryMethod === 'External' && audioStream.DeliveryUrl) {
                    externalAudioUrl = audioStream.DeliveryUrl;
                } else {
                    const relIdx = getRelativeIndexByType(streams, defaultAudioIdx, 'Audio');
                    audioParam = relIdx != null ? relIdx : MpvPlayerBase.TRACK_DISABLE;
                }
            }

            let subParam = MpvPlayerBase.TRACK_DISABLE;
            let externalSubUrl = null;
            if (defaultSubIdx >= 0) {
                const subStream = getStreamByIndex(streams, defaultSubIdx);
                if (subStream && subStream.DeliveryMethod === 'External' && subStream.DeliveryUrl) {
                    externalSubUrl = subStream.DeliveryUrl;
                } else {
                    const relIdx = getRelativeIndexByType(streams, defaultSubIdx, 'Subtitle');
                    subParam = relIdx != null ? relIdx : MpvPlayerBase.TRACK_DISABLE;
                }
            }

            return { videoParam: 1, audioParam, subParam, externalAudioUrl, externalSubUrl };
        }

        _beforeLoad(options) {
            window.api.player.setAspectMode(options?.aspectRatio || this.getAspectRatio());
        }

        setSubtitleStreamIndex(index) {
            if (index == null || index < 0) {
                window.api.player.setSubtitleStream(MpvPlayerBase.TRACK_DISABLE);
                return;
            }
            const streams = this._currentPlayOptions?.mediaSource?.MediaStreams || [];
            const stream = getStreamByIndex(streams, index);
            if (stream && stream.DeliveryMethod === 'External' && stream.DeliveryUrl) {
                window.api.player.addSubtitleStream(stream.DeliveryUrl);
                return;
            }
            const relIdx = getRelativeIndexByType(streams, index, 'Subtitle');
            window.api.player.setSubtitleStream(relIdx != null ? relIdx : MpvPlayerBase.TRACK_DISABLE);
        }

        setSecondarySubtitleStreamIndex(index) {}

        resetSubtitleOffset() {
            this._currentSubtitleOffset = 0;
            this._showSubtitleOffset = false;
            window.api.player.setSubtitleDelay(0);
        }

        enableShowingSubtitleOffset() { this._showSubtitleOffset = true; }
        disableShowingSubtitleOffset() { this._showSubtitleOffset = false; }
        isShowingSubtitleOffsetEnabled() { return this._showSubtitleOffset === true; }
        setSubtitleOffset(offset) {
            const v = parseFloat(offset) || 0;
            this._currentSubtitleOffset = v;
            window.api.player.setSubtitleDelay(Math.round(v * 1000));
        }
        getSubtitleOffset() { return this._currentSubtitleOffset || 0; }

        setAudioStreamIndex(index) {
            if (index == null || index < 0) {
                window.api.player.setAudioStream(MpvPlayerBase.TRACK_DISABLE);
                return;
            }
            const streams = this._currentPlayOptions?.mediaSource?.MediaStreams || [];
            const stream = getStreamByIndex(streams, index);
            if (stream?.IsExternal) {
                // External audio isn't part of the source container and the server
                // doesn't pre-publish a DeliveryUrl for it, so we can't audio-add
                // client-side. Re-enter playbackManager with canSetAudioStreamIndex
                // forced false so it routes through changeStream — the server then
                // regenerates the playback URL with the external audio attached.
                this._forceServerReload = true;
                try {
                    this.playbackManager.setAudioStreamIndex(index, this);
                } finally {
                    this._forceServerReload = false;
                }
                return;
            }
            const relIdx = getRelativeIndexByType(streams, index, 'Audio');
            window.api.player.setAudioStream(relIdx != null ? relIdx : MpvPlayerBase.TRACK_DISABLE);
        }

        // Empty title = OSD cleared (stop, or a player without an item).
        onOsdTitleChange(title) {
            const src = title ? osdLogoSource(this._currentPlayOptions?.item) : null;
            const url = src ? osdLogoUrl(src, this._currentSrc) : null;
            if (url) placeOsdLogo(url);
            else removeOsdLogo();
        }

        stop(destroyPlayer) {
            if (!destroyPlayer && this._videoDialog && this._currentPlayOptions?.backdropUrl) {
                const dlg = this._videoDialog;
                const url = this._currentPlayOptions.backdropUrl;
                if (!dlg.querySelector('.mpvPoster')) {
                    const poster = document.createElement('div');
                    poster.classList.add('mpvPoster');
                    poster.style.cssText = `position:absolute;top:0;left:0;right:0;bottom:0;background:#000 url('${url}') center/cover no-repeat;`;
                    dlg.appendChild(poster);
                }
            }
            window.api.player.stop();
            this.handlers.onEnded();
            if (destroyPlayer) this.destroy();
            return Promise.resolve();
        }

        removeMediaDialog() {
            window.api.player.stop();
            if (window.jmpNative) window.jmpNative.playerOsdActive(false);
            window.api.player.setVideoRectangle(-1, 0, 0, 0);
            document.body.classList.remove('hide-scroll');
            const dlg = this._videoDialog;
            if (dlg) {
                this.setTransparency(0);
                this._videoDialog = null;
                dlg.parentNode.removeChild(dlg);
            }
        }

        destroy() {
            this.removeMediaDialog();
            this.disconnectSignals();

            // Support jellyfin-web v10.10.7
            this._currentAspectRatio = undefined;
        }

        createMediaElement(options) {
            let dlg = document.querySelector('.videoPlayerContainer');
            const isNewDlg = !dlg;
            if (isNewDlg) {
                if (window.jmpNative) window.jmpNative.playerOsdActive(true);
                dlg = document.createElement('div');
                dlg.classList.add('videoPlayerContainer');
                dlg.style.cssText = 'position:fixed;top:0;bottom:0;left:0;right:0;display:flex;align-items:center;background:transparent;';
                if (options.fullscreen) dlg.style.zIndex = 1000;  // fills entire web content area, not the actual screen
                document.body.insertBefore(dlg, document.body.firstChild);
                this._videoDialog = dlg;

                this.connectSignals();
                if (window.jmpNative) {
                    window.jmpNative.notifyRateChange(this._playRate);
                }
            } else {
                this._videoDialog = dlg;
            }

            const existing = dlg.querySelector('.mpvPoster');
            if (existing) existing.remove();
            const poster = document.createElement('div');
            poster.classList.add('mpvPoster');
            const bg = options.backdropUrl
                ? `#000 url('${options.backdropUrl}') center/cover no-repeat`
                : '#000';
            poster.style.cssText = `position:absolute;top:0;left:0;right:0;bottom:0;background:${bg};`;

            const ready = new Promise((resolve) => {
                if (isNewDlg && options.fullscreen) {
                    dlg.style.animation = 'mpv-video-zoomin 240ms ease-in normal';
                    dlg.addEventListener('animationend', resolve, { once: true });
                } else {
                    resolve();
                }
            });
            if (isNewDlg) ready.then(() => this.setTransparency(2));
            dlg.appendChild(poster);

            if (options.fullscreen) document.body.classList.add('hide-scroll');  // fills entire web content area, not the actual screen
            return ready;
        }

        canPlayMediaType(mediaType) {
            return (mediaType || '').toLowerCase() === 'video';
        }
        canPlayItem(item) { return this.canPlayMediaType(item.MediaType); }
        supportsPlayMethod() { return true; }
        static getSupportedFeatures() { return ['PlaybackRate', 'SetAspectRatio', 'SubtitleOffset']; }
        supports(feature) { return mpvVideoPlayer.getSupportedFeatures().includes(feature); }
        isFullscreen() { return window._isFullscreen === true; }
        toggleFullscreen() {
            if (window.jmpNative) window.jmpNative.toggleFullscreen();
        }

        setPlaybackRate(value) {
            super.setPlaybackRate(value);
            if (window.jmpNative) window.jmpNative.notifyRateChange(value);
        }

        canSetAudioStreamIndex() { return !this._forceServerReload; }
        setPictureInPictureEnabled() {}
        isPictureInPictureEnabled() { return false; }
        isAirPlayEnabled() { return false; }
        setAirPlayEnabled() {}
        setBrightness() {}
        getBrightness() { return 100; }

        togglePictureInPicture() {}
        toggleAirPlay() {}
        getStats() {
            const categories = [];
            // Windows + RTX: surface VSR/HDR in the Playback Info panel, each on
            // its own row. Prefer mpv's real runtime outcome (pushed via
            // _nativeRtxStatus); fall back to the configured setting when mpv
            // hasn't reported yet. mpv only logs success at verbose, so without
            // verbose logging an enabled feature shows as "On"; a GPU rejection
            // is logged at warn and always surfaces as "Failed"/"Unsupported".
            if (navigator.platform.startsWith('Win')) {
                const pb = (window.jmpInfo && window.jmpInfo.settings && window.jmpInfo.settings.playback) || {};
                const rt = window.__rtxStatus || {};
                const stats = [
                    { label: 'RTX Video Super Resolution', value: rtxVsrStatus(!!pb.rtxVsr, rt.vsr) },
                    { label: 'RTX Video HDR', value: rtxHdrStatus(!!pb.rtxHdr, rt.hdr) }
                ];
                // The evidence the two rows above are read from, shown as-is so a
                // surprising verdict can be checked rather than taken on trust.
                const pipeline = describePipeline();
                if (pipeline) stats.push({ label: 'Pipeline', value: pipeline });
                const gpu = describeGpuLoad();
                if (gpu) stats.push({ label: 'GPU load', value: gpu });
                categories.push({ name: 'RTX Video Enhancement', stats });
            }
            // Directly under the RTX rows, ahead of jellyfin-web's own media info.
            const buffer = getBufferStatsCategory();
            if (buffer) categories.push(buffer);
            return Promise.resolve({ categories });
        }
        getSupportedAspectRatios() {
            return [
                { id: 'auto',  name: this.globalize.translate('Auto') },
                { id: 'cover', name: this.globalize.translate('AspectRatioCover') },
                { id: 'fill',  name: this.globalize.translate('AspectRatioFill') }
            ];
        }
        getAspectRatio() {
            const aspectRatio = typeof this.appSettings.aspectRatio === 'function'
                ? this.appSettings.aspectRatio()
                // Support jellyfin-web v10.10.7
                : this._currentAspectRatio;

            return aspectRatio || 'auto';
        }
        setAspectRatio(value) {
            if (typeof this.appSettings.aspectRatio === 'function') {
                this.appSettings.aspectRatio(value);
            } else {
                // Support jellyfin-web v10.10.7
                this._currentAspectRatio = value;
            }
            window.api.player.setAspectMode(value);
        }
    }

    window._mpvVideoPlayer = mpvVideoPlayer;
    console.debug('[Media] mpvVideoPlayer class installed');
})();
