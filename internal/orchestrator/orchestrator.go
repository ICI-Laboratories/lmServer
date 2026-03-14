package orchestrator

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"sync"
	"sync/atomic"
	"time"
)

var ErrUnhealthyUpstream = errors.New("upstream returned unhealthy response")

type slotState string

const (
	slotStateStarting slotState = "starting"
	slotStateReady    slotState = "ready"
	slotStateBusy     slotState = "busy"
	slotStatePaused   slotState = "paused"
	slotStatePausing  slotState = "pausing"
	slotStateResuming slotState = "resuming"
	slotStateRemoving slotState = "removing"
)

type containerSlot struct {
	SlotID      string
	ContainerID string
	Name        string
	Profile     ContainerProfile
	State       slotState
	TargetURL   string
	HealthURL   string
	CreatedAt   time.Time
	LastUsedAt  time.Time
}

type acquirePlan struct {
	Kind   string
	SlotID string
}

type maintenanceAction struct {
	Kind        string
	SlotID      string
	ContainerID string
	Previous    slotState
}

type Orchestrator struct {
	logger                 *slog.Logger
	docker                 *dockerManager
	httpClient             *http.Client
	totalVRAMMB            int
	classBudgetMB          map[QueueClass]int
	schedulingInterval     time.Duration
	maintenanceInterval    time.Duration
	containerReadyInterval time.Duration
	profilesByClass        map[QueueClass][]ContainerProfile
	mu                     sync.Mutex
	slots                  map[string]*containerSlot
	slotSeq                atomic.Uint64
}

type Lease struct {
	orchestrator *Orchestrator
	slotID       string
	containerID  string
	targetURL    string
	profile      ContainerProfile
}

func New(ctx context.Context, logger *slog.Logger, cfg Config) (*Orchestrator, error) {
	dockerManager, err := newDockerManager(ctx, cfg.DockerHost)
	if err != nil {
		return nil, err
	}

	classBudgetMB := make(map[QueueClass]int, len(cfg.ClassBudgetMB))
	for class, budget := range cfg.ClassBudgetMB {
		classBudgetMB[class] = budget
	}

	profilesByClass := make(map[QueueClass][]ContainerProfile)
	for _, profile := range cfg.Profiles {
		profilesByClass[profile.Class] = append(profilesByClass[profile.Class], profile)
	}

	return &Orchestrator{
		logger:                 logger,
		docker:                 dockerManager,
		httpClient:             &http.Client{},
		totalVRAMMB:            cfg.TotalVRAMMB,
		classBudgetMB:          classBudgetMB,
		schedulingInterval:     cfg.SchedulingPollInterval,
		maintenanceInterval:    cfg.MaintenanceInterval,
		containerReadyInterval: cfg.ContainerReadyInterval,
		profilesByClass:        profilesByClass,
		slots:                  make(map[string]*containerSlot),
	}, nil
}

func (o *Orchestrator) Start(ctx context.Context) {
	go o.maintenanceLoop(ctx)
}

func (o *Orchestrator) Close() error {
	return o.docker.close()
}

func (o *Orchestrator) Acquire(ctx context.Context, meta RequestMeta) (*Lease, error) {
	profile, err := o.selectProfile(meta)
	if err != nil {
		return nil, err
	}

	for {
		if err := ctx.Err(); err != nil {
			return nil, err
		}

		lease, plan, err := o.planAcquire(profile)
		if err != nil {
			return nil, err
		}
		if lease != nil {
			return lease, nil
		}

		switch plan.Kind {
		case "launch":
			lease, err = o.launchReservedSlot(ctx, plan.SlotID)
			if err == nil {
				return lease, nil
			}
			o.logger.WarnContext(ctx, "container launch failed", "profile", profile.Name, "error", err)
		case "resume":
			lease, err = o.resumeReservedSlot(ctx, plan.SlotID)
			if err == nil {
				return lease, nil
			}
			o.logger.WarnContext(ctx, "container resume failed", "profile", profile.Name, "error", err)
		}

		timer := time.NewTimer(o.schedulingInterval)
		select {
		case <-ctx.Done():
			timer.Stop()
			return nil, ctx.Err()
		case <-timer.C:
		}
	}
}

func (o *Orchestrator) Forward(ctx context.Context, lease *Lease, body []byte, headers http.Header) (*http.Response, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, lease.targetURL, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}

	copyRequestHeaders(req.Header, headers)
	if req.Header.Get("Content-Type") == "" {
		req.Header.Set("Content-Type", "application/json")
	}

	return o.httpClient.Do(req)
}

func (o *Orchestrator) Snapshot() RuntimeSnapshot {
	o.mu.Lock()
	defer o.mu.Unlock()

	snapshot := RuntimeSnapshot{
		TotalVRAMMB:  o.totalVRAMMB,
		UsedVRAMMB:   0,
		ClassUsageMB: make(map[QueueClass]int),
		Containers:   make([]ContainerSnapshot, 0, len(o.slots)),
	}

	for _, slot := range o.slots {
		if slot.State == slotStateRemoving {
			continue
		}

		snapshot.UsedVRAMMB += slot.Profile.VRAMMB
		snapshot.ClassUsageMB[slot.Profile.Class] += slot.Profile.VRAMMB
		snapshot.Containers = append(snapshot.Containers, ContainerSnapshot{
			SlotID:      slot.SlotID,
			ContainerID: slot.ContainerID,
			Name:        slot.Name,
			Profile:     slot.Profile.Name,
			Class:       slot.Profile.Class,
			Backend:     slot.Profile.Backend,
			State:       string(slot.State),
			TargetURL:   slot.TargetURL,
			VRAMMB:      slot.Profile.VRAMMB,
			CreatedAt:   slot.CreatedAt,
			LastUsedAt:  slot.LastUsedAt,
		})
	}

	return snapshot
}

func (l *Lease) Backend() Backend {
	return l.profile.Backend
}

func (l *Lease) Class() QueueClass {
	return l.profile.Class
}

func (l *Lease) ContainerID() string {
	return l.containerID
}

func (l *Lease) Release(err error) {
	l.orchestrator.release(l.slotID, err)
}

func (o *Orchestrator) selectProfile(meta RequestMeta) (ContainerProfile, error) {
	profiles := o.profilesByClass[meta.Class]
	if len(profiles) == 0 {
		return ContainerProfile{}, fmt.Errorf("no container profile for queue class %s", meta.Class)
	}

	if meta.BackendHint != "" {
		for _, profile := range profiles {
			if profile.Backend == meta.BackendHint {
				return profile, nil
			}
		}
	}

	return profiles[0], nil
}

func (o *Orchestrator) planAcquire(profile ContainerProfile) (*Lease, *acquirePlan, error) {
	o.mu.Lock()
	defer o.mu.Unlock()

	now := time.Now()

	for _, slot := range o.slots {
		if slot.Profile.Name != profile.Name {
			continue
		}
		if slot.State == slotStateReady {
			slot.State = slotStateBusy
			slot.LastUsedAt = now
			return o.newLeaseLocked(slot), nil, nil
		}
	}

	for _, slot := range o.slots {
		if slot.Profile.Name != profile.Name {
			continue
		}
		if slot.State == slotStatePaused {
			slot.State = slotStateResuming
			slot.LastUsedAt = now
			return nil, &acquirePlan{Kind: "resume", SlotID: slot.SlotID}, nil
		}
	}

	if !o.canLaunchLocked(profile) {
		return nil, &acquirePlan{Kind: "wait"}, nil
	}

	slotID := o.nextSlotID()
	slot := &containerSlot{
		SlotID:     slotID,
		Name:       fmt.Sprintf("lmserv-%s-%d", profile.Name, time.Now().UnixNano()),
		Profile:    profile,
		State:      slotStateStarting,
		CreatedAt:  now,
		LastUsedAt: now,
	}
	o.slots[slotID] = slot

	return nil, &acquirePlan{Kind: "launch", SlotID: slotID}, nil
}

func (o *Orchestrator) canLaunchLocked(profile ContainerProfile) bool {
	if o.totalSlotsForProfileLocked(profile.Name) >= profile.MaxContainers {
		return false
	}

	totalUsed := 0
	classUsed := 0
	for _, slot := range o.slots {
		if slot.State == slotStateRemoving {
			continue
		}
		totalUsed += slot.Profile.VRAMMB
		if slot.Profile.Class == profile.Class {
			classUsed += slot.Profile.VRAMMB
		}
	}

	if totalUsed+profile.VRAMMB > o.totalVRAMMB {
		return false
	}

	budget := o.classBudgetMB[profile.Class]
	if budget > 0 && classUsed+profile.VRAMMB > budget {
		return false
	}

	return true
}

func (o *Orchestrator) totalSlotsForProfileLocked(profileName string) int {
	total := 0
	for _, slot := range o.slots {
		if slot.Profile.Name == profileName && slot.State != slotStateRemoving {
			total++
		}
	}
	return total
}

func (o *Orchestrator) launchReservedSlot(ctx context.Context, slotID string) (*Lease, error) {
	o.mu.Lock()
	slot, ok := o.slots[slotID]
	if !ok || slot.State != slotStateStarting {
		o.mu.Unlock()
		return nil, fmt.Errorf("slot %s no longer available for launch", slotID)
	}
	profile := slot.Profile
	name := slot.Name
	o.mu.Unlock()

	launched, err := o.docker.launchEphemeralContainer(ctx, profile, name)
	if err != nil {
		o.mu.Lock()
		delete(o.slots, slotID)
		o.mu.Unlock()
		return nil, err
	}

	if err := o.waitUntilReady(ctx, launched.HealthURL, profile.StartTimeout); err != nil {
		removeCtx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		_ = o.docker.removeContainer(removeCtx, launched.ContainerID)
		o.mu.Lock()
		delete(o.slots, slotID)
		o.mu.Unlock()
		return nil, err
	}

	o.mu.Lock()
	defer o.mu.Unlock()

	slot, ok = o.slots[slotID]
	if !ok {
		return nil, fmt.Errorf("slot %s vanished during launch", slotID)
	}

	slot.ContainerID = launched.ContainerID
	slot.TargetURL = launched.TargetURL
	slot.HealthURL = launched.HealthURL
	slot.State = slotStateBusy
	slot.LastUsedAt = time.Now()

	return o.newLeaseLocked(slot), nil
}

func (o *Orchestrator) resumeReservedSlot(ctx context.Context, slotID string) (*Lease, error) {
	o.mu.Lock()
	slot, ok := o.slots[slotID]
	if !ok || slot.State != slotStateResuming {
		o.mu.Unlock()
		return nil, fmt.Errorf("slot %s no longer available for resume", slotID)
	}
	containerID := slot.ContainerID
	healthURL := slot.HealthURL
	profile := slot.Profile
	o.mu.Unlock()

	if err := o.docker.unpauseContainer(ctx, containerID); err != nil {
		o.destroySlot(slotID, err)
		return nil, err
	}

	if err := o.waitUntilReady(ctx, healthURL, profile.StartTimeout); err != nil {
		o.destroySlot(slotID, err)
		return nil, err
	}

	o.mu.Lock()
	defer o.mu.Unlock()

	slot, ok = o.slots[slotID]
	if !ok {
		return nil, fmt.Errorf("slot %s vanished during resume", slotID)
	}
	slot.State = slotStateBusy
	slot.LastUsedAt = time.Now()

	return o.newLeaseLocked(slot), nil
}

func (o *Orchestrator) waitUntilReady(ctx context.Context, healthURL string, timeout time.Duration) error {
	if healthURL == "" {
		return nil
	}

	readyCtx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	client := &http.Client{
		Timeout: 3 * time.Second,
	}

	for {
		req, err := http.NewRequestWithContext(readyCtx, http.MethodGet, healthURL, nil)
		if err != nil {
			return err
		}

		resp, err := client.Do(req)
		if err == nil {
			_, _ = io.Copy(io.Discard, resp.Body)
			_ = resp.Body.Close()
			if resp.StatusCode < http.StatusInternalServerError {
				return nil
			}
		}

		timer := time.NewTimer(o.containerReadyInterval)
		select {
		case <-readyCtx.Done():
			timer.Stop()
			if err != nil {
				return err
			}
			return readyCtx.Err()
		case <-timer.C:
		}
	}
}

func (o *Orchestrator) release(slotID string, err error) {
	if isFatalSlotError(err) {
		o.destroySlot(slotID, err)
		return
	}

	o.mu.Lock()
	defer o.mu.Unlock()

	slot, ok := o.slots[slotID]
	if !ok {
		return
	}
	slot.State = slotStateReady
	slot.LastUsedAt = time.Now()
}

func (o *Orchestrator) destroySlot(slotID string, cause error) {
	o.mu.Lock()
	slot, ok := o.slots[slotID]
	if !ok {
		o.mu.Unlock()
		return
	}
	slot.State = slotStateRemoving
	containerID := slot.ContainerID
	profile := slot.Profile.Name
	o.mu.Unlock()

	go func() {
		removeCtx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()

		if containerID != "" {
			if err := o.docker.removeContainer(removeCtx, containerID); err != nil {
				o.logger.Warn("container removal failed", "slot_id", slotID, "container_id", containerID, "error", err)
			}
		}

		o.mu.Lock()
		delete(o.slots, slotID)
		o.mu.Unlock()

		if cause != nil {
			o.logger.Warn("slot recycled after upstream failure", "slot_id", slotID, "profile", profile, "error", cause)
		}
	}()
}

func (o *Orchestrator) maintenanceLoop(ctx context.Context) {
	ticker := time.NewTicker(o.maintenanceInterval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			o.runMaintenance(ctx)
		}
	}
}

func (o *Orchestrator) runMaintenance(ctx context.Context) {
	actions := o.collectMaintenanceActions()
	for _, action := range actions {
		switch action.Kind {
		case "pause":
			timeoutCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
			err := o.docker.pauseContainer(timeoutCtx, action.ContainerID)
			cancel()
			o.finishPause(action, err)
		case "remove":
			timeoutCtx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
			err := o.docker.removeContainer(timeoutCtx, action.ContainerID)
			cancel()
			o.finishRemove(action, err)
		}
	}
}

func (o *Orchestrator) collectMaintenanceActions() []maintenanceAction {
	o.mu.Lock()
	defer o.mu.Unlock()

	now := time.Now()
	var actions []maintenanceAction

	for _, slot := range o.slots {
		idleFor := now.Sub(slot.LastUsedAt)
		switch slot.State {
		case slotStateReady:
			if slot.Profile.IdleDestroyAfter > 0 && idleFor >= slot.Profile.IdleDestroyAfter {
				slot.State = slotStateRemoving
				actions = append(actions, maintenanceAction{
					Kind:        "remove",
					SlotID:      slot.SlotID,
					ContainerID: slot.ContainerID,
					Previous:    slotStateReady,
				})
				continue
			}
			if slot.Profile.IdlePauseAfter > 0 && idleFor >= slot.Profile.IdlePauseAfter {
				slot.State = slotStatePausing
				actions = append(actions, maintenanceAction{
					Kind:        "pause",
					SlotID:      slot.SlotID,
					ContainerID: slot.ContainerID,
					Previous:    slotStateReady,
				})
			}
		case slotStatePaused:
			if slot.Profile.IdleDestroyAfter > 0 && idleFor >= slot.Profile.IdleDestroyAfter {
				slot.State = slotStateRemoving
				actions = append(actions, maintenanceAction{
					Kind:        "remove",
					SlotID:      slot.SlotID,
					ContainerID: slot.ContainerID,
					Previous:    slotStatePaused,
				})
			}
		}
	}

	return actions
}

func (o *Orchestrator) finishPause(action maintenanceAction, err error) {
	o.mu.Lock()
	defer o.mu.Unlock()

	slot, ok := o.slots[action.SlotID]
	if !ok {
		return
	}

	if err != nil {
		slot.State = action.Previous
		o.logger.Warn("container pause failed", "slot_id", action.SlotID, "container_id", action.ContainerID, "error", err)
		return
	}

	slot.State = slotStatePaused
}

func (o *Orchestrator) finishRemove(action maintenanceAction, err error) {
	o.mu.Lock()
	defer o.mu.Unlock()

	if err != nil {
		if slot, ok := o.slots[action.SlotID]; ok {
			slot.State = action.Previous
		}
		o.logger.Warn("container removal failed", "slot_id", action.SlotID, "container_id", action.ContainerID, "error", err)
		return
	}

	delete(o.slots, action.SlotID)
}

func (o *Orchestrator) newLeaseLocked(slot *containerSlot) *Lease {
	return &Lease{
		orchestrator: o,
		slotID:       slot.SlotID,
		containerID:  slot.ContainerID,
		targetURL:    slot.TargetURL,
		profile:      slot.Profile,
	}
}

func (o *Orchestrator) nextSlotID() string {
	return fmt.Sprintf("slot-%06d", o.slotSeq.Add(1))
}

func copyRequestHeaders(dst http.Header, src http.Header) {
	for key, values := range src {
		if isHopByHopHeader(key) {
			continue
		}
		for _, value := range values {
			dst.Add(key, value)
		}
	}
}

func isFatalSlotError(err error) bool {
	if err == nil {
		return false
	}
	if errors.Is(err, ErrUnhealthyUpstream) {
		return true
	}
	if errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
		return false
	}
	var netErr net.Error
	return !errors.As(err, &netErr) || !netErr.Timeout()
}

func isHopByHopHeader(key string) bool {
	switch http.CanonicalHeaderKey(key) {
	case "Connection", "Keep-Alive", "Proxy-Authenticate", "Proxy-Authorization", "Te", "Trailer", "Transfer-Encoding", "Upgrade":
		return true
	default:
		return false
	}
}
