import { Radio, Trash2, Users } from 'lucide-react'
import type { Bond } from '@/lib/agent'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardAction, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { DashboardEmpty } from '@/components/dashboard/dashboard-components'

export const SecurityView = ({ bonds, role, enrollmentUntil, owner, busy, onOpenEnrollment, onCloseEnrollment, onTransferOwner, onRevoke }: {
  bonds: Bond[]
  role?: string
  enrollmentUntil: number
  owner: boolean
  busy: boolean
  onOpenEnrollment: () => void
  onCloseEnrollment: () => void
  onTransferOwner: (id: string) => void
  onRevoke: (id: string) => void
}) => (
  <Card>
    <CardHeader>
      <div className="flex items-center gap-2"><CardTitle>受信主机</CardTitle><Badge variant="secondary">{role ?? '—'}</Badge></div>
      <CardAction><Button size="sm" disabled={!owner || busy} onClick={onOpenEnrollment}><Users data-icon="inline-start" />添加</Button></CardAction>
    </CardHeader>
    <CardContent className="px-0">
      {enrollmentUntil > Date.now() ? (
        <div className="mx-4 mb-4 flex items-center gap-3 rounded-lg border bg-muted/30 p-3 text-sm">
          <Radio className="size-4" />
          <span>Enrollment</span>
          <Button className="ml-auto" variant="ghost" size="sm" onClick={onCloseEnrollment}>关闭</Button>
        </div>
      ) : null}
      {bonds.length ? (
        <Table>
          <TableHeader><TableRow><TableHead>Host ID</TableHead><TableHead>角色</TableHead><TableHead className="text-right">操作</TableHead></TableRow></TableHeader>
          <TableBody>{bonds.map((bond) => (
            <TableRow key={bond.id}>
              <TableCell className="font-mono text-xs">{bond.id}</TableCell>
              <TableCell><Badge variant={bond.role === 'owner' ? 'default' : 'secondary'}>{bond.role}</Badge></TableCell>
              <TableCell><div className="flex justify-end gap-1">
                {bond.role !== 'owner' ? <Button variant="outline" size="sm" disabled={!owner || busy} onClick={() => onTransferOwner(bond.id)}>设为 Owner</Button> : null}
                <Button variant="ghost" size="icon-sm" disabled={!owner || bond.role === 'owner' || busy} onClick={() => onRevoke(bond.id)}><Trash2 /><span className="sr-only">撤销</span></Button>
              </div></TableCell>
            </TableRow>
          ))}</TableBody>
        </Table>
      ) : <DashboardEmpty>没有受信主机</DashboardEmpty>}
    </CardContent>
  </Card>
)
