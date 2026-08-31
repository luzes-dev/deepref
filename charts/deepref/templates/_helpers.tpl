{{- define "deepref.name" -}}
deepref
{{- end -}}

{{- define "deepref.fullname" -}}
{{- if eq .Release.Name "deepref" -}}deepref{{- else -}}{{ printf "%s-deepref" .Release.Name | trunc 63 | trimSuffix "-" }}{{- end -}}
{{- end -}}

{{- define "deepref.namespace" -}}
{{- default .Release.Namespace .Values.namespace.name -}}
{{- end -}}

{{- define "deepref.componentName" -}}
{{- printf "%s-%s" (include "deepref.fullname" .root) .component | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "deepref.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | quote }}
app.kubernetes.io/name: {{ include "deepref.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: deepref
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
deepref.io/environment: {{ .Values.global.environment }}
{{- end -}}

{{- define "deepref.selectorLabels" -}}
app.kubernetes.io/name: {{ include "deepref.name" .root }}
app.kubernetes.io/instance: {{ .root.Release.Name }}
app.kubernetes.io/component: {{ .component }}
{{- end -}}

{{- define "deepref.image" -}}
{{- $repository := required (printf "images.%s.repository is required" .name) .image.repository -}}
{{- $digest := required (printf "images.%s.digest is required" .name) .image.digest -}}
{{- if not (regexMatch "^sha256:[0-9a-f]{64}$" $digest) -}}
{{- fail (printf "images.%s.digest must be a sha256 digest" .name) -}}
{{- end -}}
{{- printf "%s@%s" $repository $digest -}}
{{- end -}}

{{- define "deepref.podSecurityContext" -}}
runAsNonRoot: true
seccompProfile:
  type: RuntimeDefault
{{- end -}}

{{- define "deepref.containerSecurityContext" -}}
allowPrivilegeEscalation: false
readOnlyRootFilesystem: true
capabilities:
  drop:
    - ALL
{{- end -}}

{{- define "deepref.imagePullSecrets" -}}
{{- with .Values.global.imagePullSecrets }}
imagePullSecrets:
{{- range . }}
  - name: {{ . }}
{{- end }}
{{- end }}
{{- end -}}

{{- define "deepref.topology" -}}
topologySpreadConstraints:
  - maxSkew: 1
    topologyKey: topology.kubernetes.io/zone
    whenUnsatisfiable: ScheduleAnyway
    labelSelector:
      matchLabels:
{{ include "deepref.selectorLabels" . | indent 8 }}
  - maxSkew: 1
    topologyKey: kubernetes.io/hostname
    whenUnsatisfiable: ScheduleAnyway
    labelSelector:
      matchLabels:
{{ include "deepref.selectorLabels" . | indent 8 }}
{{- end -}}

{{- define "deepref.runtimeEnv" -}}
- name: APP_ENV
  value: {{ .root.Values.global.environment | quote }}
- name: DATABASE_URL_FILE
  value: /var/run/secrets/deepref/database/url
- name: DATABASE_POOL_MIN
  value: {{ .root.Values.runtime.database.poolMin | quote }}
- name: DATABASE_POOL_MAX
  value: {{ .root.Values.runtime.database.poolMax | quote }}
- name: DATABASE_ACQUIRE_TIMEOUT_SECS
  value: {{ .root.Values.runtime.database.acquireTimeoutSeconds | quote }}
- name: DATABASE_IDLE_TIMEOUT_SECS
  value: {{ .root.Values.runtime.database.idleTimeoutSeconds | quote }}
- name: DATABASE_MAX_LIFETIME_SECS
  value: {{ .root.Values.runtime.database.maxLifetimeSeconds | quote }}
- name: OTEL_SERVICE_NAME
  value: {{ .serviceName | quote }}
- name: OTEL_EXPORTER_OTLP_ENDPOINT
  value: http://{{ include "deepref.componentName" (dict "root" .root "component" "adot") }}:4317
- name: SHUTDOWN_DEADLINE_SECS
  value: {{ .root.Values.runtime.shutdownDeadlineSeconds | quote }}
- name: DOCUMENT_STORAGE_BACKEND
  value: {{ .root.Values.documentStorage.backend | quote }}
- name: DOCUMENT_MAX_BYTES
  value: {{ .root.Values.documentStorage.maxBytes | quote }}
{{- if .root.Values.documentStorage.root }}
- name: DOCUMENT_STORAGE_ROOT
  value: {{ .root.Values.documentStorage.root | quote }}
{{- end }}
{{- $storageSecret := default (include "deepref.componentName" (dict "root" .root "component" "document-storage")) .root.Values.documentStorage.secretName }}
- name: DOCUMENT_STORAGE_ENDPOINT
  valueFrom: {secretKeyRef: {name: {{ $storageSecret }}, key: endpoint}}
- name: DOCUMENT_STORAGE_BUCKET
  valueFrom: {secretKeyRef: {name: {{ $storageSecret }}, key: bucket}}
- name: DOCUMENT_STORAGE_REGION
  valueFrom: {secretKeyRef: {name: {{ $storageSecret }}, key: region}}
- name: DOCUMENT_STORAGE_ACCESS_KEY_ID
  valueFrom: {secretKeyRef: {name: {{ $storageSecret }}, key: access_key_id}}
- name: DOCUMENT_STORAGE_SECRET_ACCESS_KEY
  valueFrom: {secretKeyRef: {name: {{ $storageSecret }}, key: secret_access_key}}
{{- end -}}

{{- define "deepref.runtimeVolumeMounts" -}}
- name: tmp
  mountPath: /tmp
- name: database-secret
  mountPath: /var/run/secrets/deepref/database
  readOnly: true
{{- end -}}

{{- define "deepref.runtimeVolumes" -}}
- name: tmp
  emptyDir:
    sizeLimit: 128Mi
- name: database-secret
  secret:
    secretName: {{ include "deepref.componentName" (dict "root" .root "component" "database") }}
    items:
      - key: url
        path: url
{{- end -}}
