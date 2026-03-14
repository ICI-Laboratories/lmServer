package orchestrator

import (
	"context"
	"fmt"
	"net/url"

	"github.com/docker/docker/api/types/container"
	"github.com/docker/docker/client"
	"github.com/docker/go-connections/nat"
)

type dockerManager struct {
	client *client.Client
}

type launchedContainer struct {
	ContainerID string
	Name        string
	BaseURL     string
	TargetURL   string
	HealthURL   string
}

func newDockerManager(ctx context.Context, dockerHost string) (*dockerManager, error) {
	opts := []client.Opt{
		client.FromEnv,
		client.WithAPIVersionNegotiation(),
	}
	if dockerHost != "" {
		opts = append(opts, client.WithHost(dockerHost))
	}

	dockerClient, err := client.NewClientWithOpts(opts...)
	if err != nil {
		return nil, err
	}

	if _, err := dockerClient.Ping(ctx); err != nil {
		_ = dockerClient.Close()
		return nil, err
	}

	return &dockerManager{client: dockerClient}, nil
}

func (d *dockerManager) close() error {
	return d.client.Close()
}

func (d *dockerManager) launchEphemeralContainer(ctx context.Context, profile ContainerProfile, name string) (*launchedContainer, error) {
	port, err := nat.NewPort("tcp", profile.ContainerPort)
	if err != nil {
		return nil, err
	}

	config := &container.Config{
		Image: profile.Image,
		Env:   mergeEnv(profile.Env, []string{"HSA_OVERRIDE_GFX_VERSION=10.3.0"}),
		Cmd:   profile.Command,
		Labels: map[string]string{
			"app":     "lmserv",
			"profile": profile.Name,
			"class":   string(profile.Class),
			"backend": string(profile.Backend),
		},
		ExposedPorts: nat.PortSet{
			port: struct{}{},
		},
	}

	hostConfig := &container.HostConfig{
		GroupAdd: []string{"video", "render"},
		PortBindings: nat.PortMap{
			port: []nat.PortBinding{{
				HostIP:   "127.0.0.1",
				HostPort: "",
			}},
		},
		Resources: container.Resources{
			Devices: []container.DeviceMapping{
				{
					PathOnHost:        "/dev/kfd",
					PathInContainer:   "/dev/kfd",
					CgroupPermissions: "rwm",
				},
				{
					PathOnHost:        "/dev/dri",
					PathInContainer:   "/dev/dri",
					CgroupPermissions: "rwm",
				},
			},
		},
	}

	created, err := d.client.ContainerCreate(ctx, config, hostConfig, nil, nil, name)
	if err != nil {
		return nil, err
	}

	if err := d.client.ContainerStart(ctx, created.ID, container.StartOptions{}); err != nil {
		_ = d.client.ContainerRemove(context.Background(), created.ID, container.RemoveOptions{Force: true, RemoveVolumes: true})
		return nil, err
	}

	inspection, err := d.client.ContainerInspect(ctx, created.ID)
	if err != nil {
		_ = d.client.ContainerRemove(context.Background(), created.ID, container.RemoveOptions{Force: true, RemoveVolumes: true})
		return nil, err
	}

	bindings := inspection.NetworkSettings.Ports[port]
	if len(bindings) == 0 || bindings[0].HostPort == "" {
		_ = d.client.ContainerRemove(context.Background(), created.ID, container.RemoveOptions{Force: true, RemoveVolumes: true})
		return nil, fmt.Errorf("container %s started without host port binding", created.ID)
	}

	baseURL := (&url.URL{
		Scheme: "http",
		Host:   "127.0.0.1:" + bindings[0].HostPort,
	}).String()

	return &launchedContainer{
		ContainerID: created.ID,
		Name:        name,
		BaseURL:     baseURL,
		TargetURL:   baseURL + profile.TargetPath,
		HealthURL:   baseURL + profile.HealthPath,
	}, nil
}

func (d *dockerManager) pauseContainer(ctx context.Context, containerID string) error {
	return d.client.ContainerPause(ctx, containerID)
}

func (d *dockerManager) unpauseContainer(ctx context.Context, containerID string) error {
	return d.client.ContainerUnpause(ctx, containerID)
}

func (d *dockerManager) removeContainer(ctx context.Context, containerID string) error {
	return d.client.ContainerRemove(ctx, containerID, container.RemoveOptions{
		Force:         true,
		RemoveVolumes: true,
	})
}

func mergeEnv(values ...[]string) []string {
	seen := make(map[string]int)
	var merged []string

	for _, group := range values {
		for _, value := range group {
			key := value
			if idx := len(value); idx > 0 {
				for i := 0; i < len(value); i++ {
					if value[i] == '=' {
						key = value[:i]
						break
					}
				}
			}

			if pos, ok := seen[key]; ok {
				merged[pos] = value
				continue
			}

			seen[key] = len(merged)
			merged = append(merged, value)
		}
	}

	return merged
}
