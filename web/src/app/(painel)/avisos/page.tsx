import { Freshness, NoData, SectionTitle } from "@/components/Common";
import { EventList } from "@/components/EventList";
import { PushToggle } from "@/components/PushToggle";
import { getLive } from "@/lib/data";

export default async function AlertsPage() {
  const live = await getLive();
  return (
    <div>
      <PushToggle />
      <p className="mt-2 text-sm">
        A notificação no celular chega só para os avisos importantes (os mesmos que vão para o Telegram). Todo o resto fica aqui.
      </p>
      <SectionTitle>Avisos</SectionTitle>
      {live ? (
        <>
          <Freshness ms={live.generated_ms} label="Avisos" />
          <EventList events={live.events} />
        </>
      ) : (
        <NoData />
      )}
    </div>
  );
}
