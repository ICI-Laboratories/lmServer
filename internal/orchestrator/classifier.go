package orchestrator

import (
	"encoding/base64"
	"encoding/json"
	"strings"
)

func InspectRequest(body []byte) RequestMeta {
	meta := RequestMeta{
		Class: QueueClassLowLatency,
	}

	var payload any
	if err := json.Unmarshal(body, &payload); err != nil {
		return meta
	}

	if backend := extractBackendHint(payload); backend != "" {
		meta.BackendHint = backend
	}

	if containsEncodedImage(payload) {
		meta.Class = QueueClassHighLatency
		meta.HasImages = true
		if meta.BackendHint == "" {
			meta.BackendHint = BackendVLLM
		}
	}

	return meta
}

func extractBackendHint(payload any) Backend {
	switch value := payload.(type) {
	case map[string]any:
		for key, item := range value {
			lowerKey := strings.ToLower(key)
			if lowerKey == "backend" || lowerKey == "engine" || lowerKey == "runtime" {
				if backend, ok := item.(string); ok {
					switch strings.ToLower(strings.TrimSpace(backend)) {
					case "llama.cpp", "llama", "llamacpp":
						return BackendLlamaCPP
					case "vllm":
						return BackendVLLM
					}
				}
			}
			if backend := extractBackendHint(item); backend != "" {
				return backend
			}
		}
	case []any:
		for _, item := range value {
			if backend := extractBackendHint(item); backend != "" {
				return backend
			}
		}
	}

	return ""
}

func containsEncodedImage(payload any) bool {
	switch value := payload.(type) {
	case map[string]any:
		for key, item := range value {
			lowerKey := strings.ToLower(key)
			if strings.Contains(lowerKey, "image") || strings.Contains(lowerKey, "img") {
				if containsEncodedImage(item) {
					return true
				}
			}
			if containsEncodedImage(item) {
				return true
			}
		}
	case []any:
		for _, item := range value {
			if containsEncodedImage(item) {
				return true
			}
		}
	case string:
		return looksLikeEncodedImage(value)
	}

	return false
}

func looksLikeEncodedImage(value string) bool {
	value = strings.TrimSpace(value)
	if value == "" {
		return false
	}

	if strings.HasPrefix(value, "data:image/") {
		return true
	}

	if idx := strings.Index(value, "base64,"); idx >= 0 {
		value = value[idx+len("base64,"):]
	}

	if len(value) < 64 {
		return false
	}

	segment := value
	if len(segment) > 256 {
		segment = segment[:256]
	}

	segment = strings.TrimRight(segment, "=")
	segment += strings.Repeat("=", (4-len(segment)%4)%4)

	decoded, err := base64.StdEncoding.DecodeString(segment)
	if err != nil {
		return false
	}

	return hasKnownImageMagic(decoded)
}

func hasKnownImageMagic(decoded []byte) bool {
	switch {
	case len(decoded) >= 3 && decoded[0] == 0xff && decoded[1] == 0xd8 && decoded[2] == 0xff:
		return true
	case len(decoded) >= 8 &&
		decoded[0] == 0x89 &&
		decoded[1] == 0x50 &&
		decoded[2] == 0x4e &&
		decoded[3] == 0x47:
		return true
	case len(decoded) >= 4 &&
		decoded[0] == 'G' &&
		decoded[1] == 'I' &&
		decoded[2] == 'F' &&
		decoded[3] == '8':
		return true
	case len(decoded) >= 12 &&
		decoded[0] == 'R' &&
		decoded[1] == 'I' &&
		decoded[2] == 'F' &&
		decoded[3] == 'F' &&
		decoded[8] == 'W' &&
		decoded[9] == 'E' &&
		decoded[10] == 'B' &&
		decoded[11] == 'P':
		return true
	default:
		return false
	}
}
